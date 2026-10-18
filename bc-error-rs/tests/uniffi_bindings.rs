//! The `uniffi` feature's binding surface, as `src/ffi.rs` spells it.
//!
//! Not ported from the C++ suite: the original has no FFI layer. What these
//! tests exist to catch is the failure a compile cannot: the boundary mirror
//! in `ffi` drifting from the Rust model it
//! stands for, silently losing a variant, a nested error, or the key ordering
//! a dictionary must keep across it — and the one this boundary was redesigned
//! around, a foreign value deep enough to overflow the stack.
#![cfg(feature = "uniffi")]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use bc_error::{
    AnyError, AnyErrorKind, ErrorFacts, MetadataDictionary, MetadataDictionaryExt, MetadataType,
    MetadataValue,
    ffi::{
        self, CapturedFacts, ForeignError, ForeignErrorFacts, ForeignMetadata,
        ForeignMetadataEntry, ForeignMetadataNode,
    },
    make_any_error,
};

/// A value exercising every arm, including both recursive ones.
fn every_arm() -> MetadataValue {
    let nested = MetadataDictionary::new()
        .with("zulu", MetadataValue::Int64(-1))
        .with("alpha", MetadataValue::Boolean(true));

    MetadataValue::Array(vec![
        MetadataValue::Null,
        MetadataValue::Boolean(false),
        MetadataValue::Int64(i64::MIN),
        MetadataValue::Uint64(u64::MAX),
        MetadataValue::Float64(2.5),
        MetadataValue::String("text".to_owned()),
        MetadataValue::Dictionary(nested),
        MetadataValue::Error(Box::new(AnyError::from_kind(AnyErrorKind::new("io", 7)))),
    ])
}

/// A stream of `depth` nested single-element arrays around one integer.
fn nested_arrays(depth: usize) -> ForeignMetadata {
    let mut nodes: Vec<ForeignMetadataNode> = (0..depth)
        .map(|_| ForeignMetadataNode::Array { count: 1 })
        .collect();
    nodes.push(ForeignMetadataNode::Int64 { value: 7 });
    ForeignMetadata { nodes }
}

#[test]
fn every_variant_survives_a_round_trip_through_the_boundary_form() {
    let original = every_arm();
    let crossed = ffi::metadata_of(&original);
    let returned = ffi::value_of(&crossed);

    assert_eq!(returned, original);
}

#[test]
fn a_dictionary_crosses_in_key_order() {
    // The ordering is explicit at the boundary rather than left to a map's
    // iteration order on the far side, because `stringify` and
    // `format_message` depend on it.
    let metadata = MetadataDictionary::new()
        .with("zulu", MetadataValue::Int64(3))
        .with("alpha", MetadataValue::Int64(1))
        .with("mike", MetadataValue::Int64(2));

    let crossed = ffi::metadata_of_dictionary(&metadata);
    let ForeignMetadataNode::Dictionary { keys } = &crossed.nodes[0] else {
        panic!("a dictionary must cross rooted at a dictionary node");
    };

    assert_eq!(keys, &["alpha", "mike", "zulu"]);
    // The values follow in the same order, so the pairing is positional.
    assert_eq!(
        crossed.nodes[1..],
        [
            ForeignMetadataNode::Int64 { value: 1 },
            ForeignMetadataNode::Int64 { value: 2 },
            ForeignMetadataNode::Int64 { value: 3 },
        ]
    );
}

#[test]
fn a_duplicate_key_from_the_far_side_keeps_the_last_occurrence() {
    // This side always emits unique keys; the foreign side is not trusted to.
    let stream = ForeignMetadata {
        nodes: vec![
            ForeignMetadataNode::Dictionary {
                keys: vec!["same".to_owned(), "same".to_owned()],
            },
            ForeignMetadataNode::Int64 { value: 1 },
            ForeignMetadataNode::Int64 { value: 2 },
        ],
    };

    let rebuilt = ffi::dictionary_of(&stream);

    assert_eq!(rebuilt.len(), 1);
    assert_eq!(rebuilt.get("same"), Some(&MetadataValue::Int64(2)));
}

#[test]
fn a_nested_error_keeps_its_metadata_across_the_boundary() {
    let inner = AnyError::from_metadata(
        AnyErrorKind::new("database", 42),
        MetadataDictionary::new().with("table", MetadataValue::String("orders".to_owned())),
    );
    let crossed = ffi::metadata_of(&MetadataValue::Error(Box::new(inner.clone())));
    let MetadataValue::Error(returned) = ffi::value_of(&crossed) else {
        panic!("the Error arm did not survive the crossing");
    };

    assert_eq!(returned.category(), inner.category());
    assert_eq!(returned.kind(), inner.kind());
    assert_eq!(returned.metadata(), inner.metadata());
}

/// Facts a foreign caller would supply, counting how often each is read.
struct CountingFacts {
    reads: AtomicUsize,
}

impl ForeignErrorFacts for CountingFacts {
    fn category(&self) -> String {
        self.reads.fetch_add(1, Ordering::Relaxed);
        "network".to_owned()
    }
    fn kind(&self) -> i32 {
        self.reads.fetch_add(1, Ordering::Relaxed);
        13
    }
    fn location(&self) -> String {
        self.reads.fetch_add(1, Ordering::Relaxed);
        "somewhere.swift:9".to_owned()
    }
    fn metadata(&self) -> ForeignMetadata {
        self.reads.fetch_add(1, Ordering::Relaxed);
        ffi::metadata_dictionary(vec![ForeignMetadataEntry {
            key: "host".to_owned(),
            value: ffi::metadata_string("example-host".to_owned()),
        }])
    }
    fn context(&self) -> Vec<String> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        vec!["opening the socket".to_owned()]
    }
}

#[test]
fn capturing_foreign_facts_reads_each_field_exactly_once() {
    // Each accessor is a boundary crossing, and a snapshot that re-read them
    // could also observe an object that changed between two calls.
    let source = CountingFacts {
        reads: AtomicUsize::new(0),
    };
    let captured = CapturedFacts::capture(&source);

    assert_eq!(source.reads.load(Ordering::Relaxed), 5);
    assert_eq!(captured.category(), "network");
    assert_eq!(captured.kind(), 13);
    assert_eq!(captured.context(), ["opening the socket"]);
    assert_eq!(
        captured.metadata().get("host"),
        Some(&MetadataValue::String("example-host".to_owned()))
    );
    // Reading the snapshot back does not reach the foreign object again.
    assert_eq!(source.reads.load(Ordering::Relaxed), 5);
}

#[test]
fn a_foreign_implementor_builds_the_same_error_a_rust_one_does() {
    let source = CountingFacts {
        reads: AtomicUsize::new(0),
    };
    let native = make_any_error(&CapturedFacts::capture(&source));

    let crossed = ffi::error_from_foreign_facts(Arc::new(CountingFacts {
        reads: AtomicUsize::new(0),
    }));

    assert_eq!(crossed, ForeignError::from(&native));
    assert_eq!(AnyError::from(crossed), native);
}

#[test]
fn the_exported_renderers_agree_with_the_functions_they_stand_in_for() {
    let error = AnyError::from_metadata(
        AnyErrorKind::new("io", 5),
        MetadataDictionary::new().with("path", MetadataValue::String("/tmp/x".to_owned())),
    );

    assert_eq!(
        ffi::format_error_message(ForeignError::from(&error)),
        bc_error::format_error_message(&error)
    );

    let value = every_arm();
    assert_eq!(
        ffi::stringify_metadata_value(ffi::metadata_of(&value)),
        value.stringify()
    );
    assert_eq!(
        ffi::metadata_value_type(ffi::metadata_of(&value)),
        MetadataType::Array
    );
}

#[test]
fn a_stream_deep_enough_to_crash_the_old_tree_shape_is_refused_without_recursing() {
    // 200_000 is measured, not chosen for effect: the recursive mirror this
    // boundary used to expose survives 50_000 and aborts at 200_000 with
    // `fatal runtime error: stack overflow` — while *dropping* the value, which
    // is unwinding no caller can catch. Nothing here recurses, so the same input
    // is a refusal and the vector drops flat.
    let stream = nested_arrays(200_000);

    assert_eq!(
        ffi::metadata_rejection(stream.clone()).as_deref(),
        Some("<metadata nested deeper than the boundary converts>")
    );
    // Quoted, because the refusal *is* a string value and this crate's
    // `stringify` quotes strings — the marker is not a special case.
    assert_eq!(
        ffi::stringify_metadata_value(stream),
        "\"<metadata nested deeper than the boundary converts>\""
    );
}

#[test]
fn the_ceiling_is_exactly_where_it_says_it_is() {
    assert!(ffi::metadata_rejection(nested_arrays(ffi::MAX_METADATA_DEPTH)).is_none());
    assert!(ffi::metadata_rejection(nested_arrays(ffi::MAX_METADATA_DEPTH + 1)).is_some());
}

#[test]
fn a_stream_within_the_ceiling_still_converts() {
    let stream = nested_arrays(ffi::MAX_METADATA_DEPTH - 1);

    assert!(ffi::metadata_rejection(stream.clone()).is_none());
    assert!(ffi::stringify_metadata_value(stream).contains('7'));
}

#[test]
fn a_miscounted_stream_is_refused_rather_than_panicking() {
    // Every way a stream can fail to be exactly one rooted value. `build`
    // indexes the node vector directly, so a stream that reached it truncated
    // would panic — which is why nothing calls it without validating first.
    let truncated = ForeignMetadata {
        nodes: vec![ForeignMetadataNode::Array { count: 2 }],
    };
    let trailing = ForeignMetadata {
        nodes: vec![
            ForeignMetadataNode::Null,
            ForeignMetadataNode::Int64 { value: 1 },
        ],
    };
    let empty = ForeignMetadata { nodes: Vec::new() };
    let absurd_count = ForeignMetadata {
        nodes: vec![ForeignMetadataNode::Array { count: u64::MAX }],
    };

    for stream in [truncated, trailing, absurd_count] {
        assert_eq!(
            ffi::metadata_rejection(stream.clone()).as_deref(),
            Some("<metadata stream is malformed>"),
        );
        // Total, not panicking: the refusal is the value.
        assert_eq!(
            ffi::stringify_metadata_value(stream),
            "\"<metadata stream is malformed>\""
        );
    }
    assert_eq!(
        ffi::metadata_rejection(empty).as_deref(),
        Some("<metadata stream is empty>")
    );
}

#[test]
fn an_unusable_stream_reaches_a_dictionary_as_a_visible_refusal() {
    // Dropping is permitted, hiding the drop is not. An empty dictionary
    // would be indistinguishable from a foreign caller that sent none.
    let rebuilt = ffi::dictionary_of(&ForeignMetadata { nodes: Vec::new() });

    assert_eq!(rebuilt.len(), 1);
    assert_eq!(
        rebuilt.get(ffi::REJECTED_KEY),
        Some(&MetadataValue::String(
            "<metadata stream is empty>".to_owned()
        ))
    );
}

#[test]
fn the_constructors_compose_into_a_stream_that_decodes_to_what_was_built() {
    // The ergonomic path: a foreign author nests calls and never sees an index.
    let built = ffi::metadata_dictionary(vec![
        ForeignMetadataEntry {
            key: "zulu".to_owned(),
            value: ffi::metadata_array(vec![
                ffi::metadata_null(),
                ffi::metadata_boolean(true),
                ffi::metadata_string("deep".to_owned()),
            ]),
        },
        ForeignMetadataEntry {
            key: "alpha".to_owned(),
            value: ffi::metadata_int64(-5),
        },
    ]);

    assert!(ffi::metadata_rejection(built.clone()).is_none());
    let expected = MetadataDictionary::new()
        .with("alpha", MetadataValue::Int64(-5))
        .with(
            "zulu",
            MetadataValue::Array(vec![
                MetadataValue::Null,
                MetadataValue::Boolean(true),
                MetadataValue::String("deep".to_owned()),
            ]),
        );

    assert_eq!(ffi::dictionary_of(&built), expected);
}

#[test]
fn the_exported_functions_agree_about_a_stream_they_refuse() {
    // A tag read that skipped validation was cheaper and wrong: it answered
    // `Array` for a stream `stringify_metadata_value` refused, and `Null` for an
    // empty one that decodes to a marker — two exported functions disagreeing
    // about one value. A refused stream converts to its marker, and a marker is
    // a string, so that is what its type is.
    let refused = [
        nested_arrays(ffi::MAX_METADATA_DEPTH + 5),
        ForeignMetadata { nodes: Vec::new() },
        ForeignMetadata {
            nodes: vec![ForeignMetadataNode::Array { count: 2 }],
        },
    ];

    for stream in refused {
        assert!(ffi::metadata_rejection(stream.clone()).is_some());
        assert_eq!(
            ffi::metadata_value_type(stream.clone()),
            MetadataType::String
        );
        assert!(matches!(ffi::value_of(&stream), MetadataValue::String(_)));
    }

    // A stream it accepts still reports its own root's tag.
    assert_eq!(
        ffi::metadata_value_type(nested_arrays(ffi::MAX_METADATA_DEPTH)),
        MetadataType::Array
    );
}
