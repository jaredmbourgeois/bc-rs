// The test doubles below return string literals from `&self` methods by design:
// they stand in for real error types whose fields are owned, and widening their
// return type to `&'static str` would stop them modelling that.
#![allow(clippy::unnecessary_literal_bound)]

use std::collections::BTreeMap;

use bc_error::{
    AnyError, AnyErrorKind, ErrorFacts, LOCATION_SEPARATOR, MetadataDictionary, MetadataValue,
    error_detail, error_detail_view, fail, format_error_message,
    format_error_message_with_location, make_any_error, prefixed_error_detail,
};

struct TestError {
    category: String,
    kind: i32,
    location: String,
    metadata: MetadataDictionary,
}

impl ErrorFacts for TestError {
    fn category(&self) -> &str {
        &self.category
    }

    fn kind(&self) -> i32 {
        self.kind
    }

    fn location(&self) -> &str {
        &self.location
    }

    fn metadata(&self) -> &MetadataDictionary {
        &self.metadata
    }
}

#[test]
fn any_error_at_location_preserves_location() {
    let error = AnyError::at_location(
        AnyErrorKind::new("network", 404),
        "file.cpp > func > #10",
        MetadataDictionary::new(),
    );
    assert_eq!(error.category(), "network");
    assert_eq!(error.kind(), 404);
    assert_eq!(error.location(), "file.cpp > func > #10");
    assert!(error.metadata().is_empty());
}

#[test]
fn any_error_at_location_preserves_metadata() {
    let metadata = BTreeMap::from([(String::from("url"), MetadataValue::from("/api/users"))]);
    let error = AnyError::at_location(
        AnyErrorKind::new("network", 404),
        "file.cpp > func > #10",
        metadata,
    );

    assert_eq!(error.metadata().len(), 1);
    assert_eq!(error.metadata()["url"].as_string(), Some("/api/users"));
}

#[test]
fn any_error_source_location_construction() {
    let expected_line = line!() + 1;
    let error = AnyError::from_kind(AnyErrorKind::new("parse", 1));
    assert_eq!(error.category(), "parse");
    assert_eq!(error.kind(), 1);
    assert!(error.location().contains("tests/error.rs"));
    assert!(error.location().contains(LOCATION_SEPARATOR));
    assert!(error.location().contains(&format!("#{expected_line}:")));
}

#[test]
fn any_error_source_location_with_metadata() {
    let metadata = BTreeMap::from([(String::from("input"), MetadataValue::from("bad json"))]);
    let expected_line = line!() + 1;
    let error = AnyError::from_metadata(AnyErrorKind::new("parse", 2), metadata);

    assert_eq!(error.category(), "parse");
    assert_eq!(error.kind(), 2);
    assert!(error.location().contains(&format!("#{expected_line}:")));
    assert_eq!(error.metadata().len(), 1);
    assert_eq!(error.metadata()["input"].as_string(), Some("bad json"));
}

#[test]
fn any_error_equality() {
    let first = AnyError::at_location(
        AnyErrorKind::new("net", 404),
        "loc",
        MetadataDictionary::new(),
    );
    let same = AnyError::at_location(
        AnyErrorKind::new("net", 404),
        "loc",
        MetadataDictionary::new(),
    );
    let different = AnyError::at_location(
        AnyErrorKind::new("net", 500),
        "loc",
        MetadataDictionary::new(),
    );
    assert_eq!(first, same);
    assert_ne!(first, different);
}

#[test]
fn any_error_matches_requires_category_and_value() {
    // The pairing is the whole reason `AnyErrorKind` exists: `HTTP_NOT_FOUND`
    // carries the same 404 under a different category, and a check that read
    // only `kind()` would call it a match.
    const NET_NOT_FOUND: AnyErrorKind = AnyErrorKind::new("net", 404);
    const NET_SERVER_ERROR: AnyErrorKind = AnyErrorKind::new("net", 500);
    const HTTP_NOT_FOUND: AnyErrorKind = AnyErrorKind::new("http", 404);

    let error = AnyError::at_location(NET_NOT_FOUND, "loc", MetadataDictionary::new());

    assert!(error.matches(&NET_NOT_FOUND));
    assert!(!error.matches(&NET_SERVER_ERROR));
    assert!(!error.matches(&HTTP_NOT_FOUND));
}

#[test]
fn error_trait_make_any_error_from_custom_type() {
    let source = TestError {
        category: String::from("custom"),
        kind: 7,
        location: String::from("test.cpp > test > #1"),
        metadata: MetadataDictionary::new(),
    };
    let error = make_any_error(&source);

    assert_eq!(error.category(), "custom");
    assert_eq!(error.kind(), 7);
    assert_eq!(error.location(), "test.cpp > test > #1");
    assert!(error.context().is_empty(), "{:?}", error.context());
}

#[test]
fn any_error_context_empty_by_default() {
    let error = AnyError::at_location(
        AnyErrorKind::new("net", 404),
        "loc",
        MetadataDictionary::new(),
    );
    assert!(error.context().is_empty(), "{:?}", error.context());
}

#[test]
fn any_error_with_context_returns_new_error() {
    let original = AnyError::at_location(
        AnyErrorKind::new("net", 404),
        "loc",
        MetadataDictionary::new(),
    );
    let with_context = original.clone().with_context("fetching user profile");

    assert!(original.context().is_empty(), "{:?}", original.context());
    assert_eq!(with_context.context(), ["fetching user profile"]);
}

#[test]
fn any_error_with_context_preserves_fields() {
    let metadata = BTreeMap::from([(String::from("url"), MetadataValue::from("/api"))]);
    let original = AnyError::at_location(
        AnyErrorKind::new("net", 404),
        "file.cpp > func > #1",
        metadata,
    );
    let with_context = original.with_context("outer");

    assert_eq!(with_context.category(), "net");
    assert_eq!(with_context.kind(), 404);
    assert_eq!(with_context.location(), "file.cpp > func > #1");
    assert_eq!(with_context.metadata().len(), 1);
    assert_eq!(with_context.metadata()["url"].as_string(), Some("/api"));
}

#[test]
fn any_error_with_context_prepends_most_recent_first() {
    let chained =
        AnyError::at_location(AnyErrorKind::new("db", 1), "loc", MetadataDictionary::new())
            .with_context("inner")
            .with_context("outer");
    assert_eq!(chained.context(), ["outer", "inner"]);
}

#[test]
fn any_error_with_context_immutability() {
    let original = AnyError::at_location(
        AnyErrorKind::new("net", 1),
        "loc",
        MetadataDictionary::new(),
    );
    let first = original.clone().with_context("first");
    let second = original.clone().with_context("second");

    assert!(original.context().is_empty(), "{:?}", original.context());
    assert_eq!(first.context(), ["first"]);
    assert_eq!(second.context(), ["second"]);
}

#[test]
fn any_error_with_metadata_returns_new_error() {
    let original = AnyError::at_location(
        AnyErrorKind::new("net", 1),
        "loc",
        MetadataDictionary::new(),
    );
    let with_metadata = original
        .clone()
        .with_metadata("key", MetadataValue::from("val"));

    assert!(original.metadata().is_empty());
    assert_eq!(with_metadata.metadata().len(), 1);
    assert_eq!(with_metadata.metadata()["key"].as_string(), Some("val"));
}

#[test]
fn any_error_with_metadata_preserves_existing() {
    let metadata = BTreeMap::from([(String::from("existing"), MetadataValue::from(42_i64))]);
    let original = AnyError::at_location(AnyErrorKind::new("net", 1), "loc", metadata);
    let with_metadata = original.with_metadata("new_key", MetadataValue::from("new_val"));

    assert_eq!(with_metadata.metadata().len(), 2);
    assert_eq!(with_metadata.metadata()["existing"].as_int64(), Some(42));
    assert_eq!(
        with_metadata.metadata()["new_key"].as_string(),
        Some("new_val")
    );
}

#[test]
fn any_error_with_metadata_overwrites_key() {
    let metadata = BTreeMap::from([(String::from("key"), MetadataValue::from(1_i64))]);
    let original = AnyError::at_location(AnyErrorKind::new("net", 1), "loc", metadata);
    let overwritten = original
        .clone()
        .with_metadata("key", MetadataValue::from(2_i64));

    assert_eq!(original.metadata()["key"].as_int64(), Some(1));
    assert_eq!(overwritten.metadata()["key"].as_int64(), Some(2));
}

#[test]
fn any_error_equality_includes_context() {
    let original = AnyError::at_location(
        AnyErrorKind::new("net", 1),
        "loc",
        MetadataDictionary::new(),
    );
    assert_ne!(original, original.clone().with_context("ctx"));
}

#[test]
fn any_error_equality_with_matching_context() {
    let original = AnyError::at_location(
        AnyErrorKind::new("net", 1),
        "loc",
        MetadataDictionary::new(),
    );
    assert_eq!(
        original.clone().with_context("ctx"),
        original.with_context("ctx")
    );
}

#[test]
fn any_error_chaining_context_and_metadata() {
    let chained =
        AnyError::at_location(AnyErrorKind::new("db", 1), "loc", MetadataDictionary::new())
            .with_context("reserving inventory")
            .with_metadata("sku_id", MetadataValue::from("SKU-001"));

    assert_eq!(chained.context(), ["reserving inventory"]);
    assert_eq!(chained.metadata().len(), 1);
    assert_eq!(chained.metadata()["sku_id"].as_string(), Some("SKU-001"));
}

#[test]
fn any_error_make_any_error_then_chain() {
    let source = TestError {
        category: String::from("custom"),
        kind: 7,
        location: String::from("test.cpp > test > #1"),
        metadata: MetadataDictionary::new(),
    };
    let chained = make_any_error(&source)
        .with_context("during checkout")
        .with_metadata("order_id", MetadataValue::from("ORD-123"));

    assert_eq!(chained.category(), "custom");
    assert_eq!(chained.kind(), 7);
    assert_eq!(chained.context(), ["during checkout"]);
    assert_eq!(chained.metadata().len(), 1);
    assert_eq!(chained.metadata()["order_id"].as_string(), Some("ORD-123"));
}

#[test]
fn any_error_format_uses_detail_metadata() {
    let metadata = BTreeMap::from([(
        String::from("detail"),
        MetadataValue::from("posix_spawnp failed: No such file"),
    )]);
    let error = AnyError::at_location(
        AnyErrorKind::new("process", 4),
        "spawn.cpp > run > #9",
        metadata,
    );

    assert_eq!(
        format_error_message(&error),
        "process kind=4: posix_spawnp failed: No such file"
    );
    assert_eq!(
        error.to_string(),
        "process kind=4: posix_spawnp failed: No such file"
    );
}

#[test]
fn any_error_format_uses_reason_metadata_when_detail_absent() {
    let metadata = BTreeMap::from([(
        String::from("reason"),
        MetadataValue::from("no mesh bodies"),
    )]);
    let error = AnyError::at_location(
        AnyErrorKind::new("viewer", 2),
        "load.cpp > run > #5",
        metadata,
    );
    assert_eq!(
        format_error_message(&error),
        "viewer kind=2: no mesh bodies"
    );
}

#[test]
fn any_error_format_skips_empty_metadata_messages() {
    let metadata = BTreeMap::from([
        (String::from("detail"), MetadataValue::from("")),
        (
            String::from("reason"),
            MetadataValue::from("no mesh bodies"),
        ),
    ]);
    let error = AnyError::at_location(
        AnyErrorKind::new("viewer", 2),
        "load.cpp > run > #5",
        metadata,
    );
    assert_eq!(
        format_error_message(&error),
        "viewer kind=2: no mesh bodies"
    );
}

#[test]
fn any_error_format_includes_context_outermost_first() {
    let metadata = BTreeMap::from([(
        String::from("detail"),
        MetadataValue::from("connection closed"),
    )]);
    let error = AnyError::at_location(
        AnyErrorKind::new("db", 3),
        "query.cpp > read > #7",
        metadata,
    )
    .with_context("read catalog")
    .with_context("startup");

    assert_eq!(
        format_error_message(&error),
        "startup: read catalog: db kind=3: connection closed"
    );
}

// The split's whole point: an error that states no reason stops at its
// identity, because a response body may carry this string to a requester. It
// used to append the source position, so any text-less error served a path.
#[test]
fn any_error_format_states_no_location_when_the_error_states_no_reason() {
    let error = AnyError::at_location(
        AnyErrorKind::new("parse", 8),
        "file.cpp > func > #12",
        MetadataDictionary::new(),
    );
    assert_eq!(format_error_message(&error), "parse kind=8");
}

#[test]
fn the_operator_formatter_falls_back_to_location() {
    let error = AnyError::at_location(
        AnyErrorKind::new("parse", 8),
        "file.cpp > func > #12",
        MetadataDictionary::new(),
    );
    assert_eq!(
        format_error_message_with_location(&error),
        "parse kind=8 @ file.cpp > func > #12"
    );
}

// The location is a last resort, not an addition: a stated reason says more
// than a source position, and appending both would put a path into every
// operator line that already explains itself.
#[test]
fn the_operator_formatter_prefers_a_stated_reason_over_the_location() {
    let metadata = BTreeMap::from([(
        String::from("detail"),
        MetadataValue::from("unterminated string"),
    )]);
    let error = AnyError::at_location(
        AnyErrorKind::new("parse", 8),
        "file.cpp > func > #12",
        metadata,
    );
    assert_eq!(
        format_error_message_with_location(&error),
        "parse kind=8: unterminated string"
    );
}

// Both formatters agree on everything but that tail, which is what makes the
// safe one a drop-in at every existing call site.
#[test]
fn both_formatters_agree_when_a_reason_is_stated() {
    let metadata = BTreeMap::from([(
        String::from("detail"),
        MetadataValue::from("connection closed"),
    )]);
    let error = AnyError::at_location(
        AnyErrorKind::new("db", 3),
        "query.cpp > read > #7",
        metadata,
    )
    .with_context("startup");
    assert_eq!(
        format_error_message(&error),
        format_error_message_with_location(&error)
    );
}

// The facts stay on both sides of the split: an operator greps `sqlstate=`, and
// they are structured codes rather than anything positional.
#[test]
fn the_safe_formatter_keeps_the_structured_facts() {
    let metadata = BTreeMap::from([(String::from("sqlstate"), MetadataValue::from("40P01"))]);
    let error = AnyError::at_location(
        AnyErrorKind::new("db", 3),
        "query.cpp > read > #7",
        metadata,
    );
    assert_eq!(format_error_message(&error), "db kind=3 sqlstate=40P01");
}

// The diagnostic reads `detail` as `error_detail` does, so it agrees with a
// user-facing message about what counts as a reason: a `reason` wins over a
// detail that is not prose.
#[test]
fn a_non_string_detail_is_not_a_stated_reason() {
    let metadata = BTreeMap::from([
        (String::from("detail"), MetadataValue::from(42_i64)),
        (
            String::from("reason"),
            MetadataValue::from("port out of range"),
        ),
    ]);
    let error = AnyError::at_location(
        AnyErrorKind::new("config", 5),
        "file.cpp > func > #12",
        metadata,
    );
    assert_eq!(
        format_error_message(&error),
        "config kind=5: port out of range"
    );
}

#[test]
fn any_error_format_stringifies_non_string_message_metadata() {
    let metadata = BTreeMap::from([(String::from("message"), MetadataValue::from(42_i64))]);
    let error = AnyError::at_location(
        AnyErrorKind::new("config", 5),
        "file.cpp > func > #12",
        metadata,
    );
    assert_eq!(format_error_message(&error), "config kind=5: 42");
}

#[test]
fn any_error_format_sanitizes_context_and_category() {
    let error = AnyError::at_location(
        AnyErrorKind::new("café\\category\nnext", 6),
        "unused",
        MetadataDictionary::new(),
    )
    .with_context("outer\tcontext")
    .with_context("alert\u{7}");

    assert_eq!(
        format_error_message(&error),
        "alert\\u{7}: outer\\tcontext: café\\\\category\\nnext kind=6"
    );
}

#[test]
fn any_error_format_sanitizes_selected_strings_and_disambiguates_backslashes() {
    for key in ["detail", "reason", "message"] {
        let metadata = BTreeMap::from([(
            String::from(key),
            MetadataValue::from("literal\\n / actual\n / carriage\r / tab\t / café"),
        )]);
        let error = AnyError::at_location(AnyErrorKind::new("format", 1), "unused", metadata);

        assert_eq!(
            format_error_message(&error),
            "format kind=1: literal\\\\n / actual\\n / carriage\\r / tab\\t / café"
        );
    }
}

#[test]
fn any_error_format_sanitizes_recursive_non_string_metadata() {
    let nested_dictionary = BTreeMap::from([(
        String::from("key\n"),
        MetadataValue::from(vec![
            MetadataValue::from("value\t"),
            MetadataValue::from(BTreeMap::from([(
                String::from("bell"),
                MetadataValue::from("x\u{7}"),
            )])),
        ]),
    )]);
    let metadata = BTreeMap::from([(
        String::from("message"),
        MetadataValue::from(nested_dictionary),
    )]);
    let error = AnyError::at_location(AnyErrorKind::new("outer", 2), "unused", metadata);

    assert_eq!(
        format_error_message(&error),
        r#"outer kind=2: {"key\n": ["value\t", {"bell": "x\u{7}"}]}"#
    );
}

#[test]
fn the_operator_formatter_sanitizes_the_fallback_location() {
    let error = AnyError::at_location(
        AnyErrorKind::new("fallback", 3),
        "path\\name\nline\rreturn\ttab\u{85}next\u{2028}line\u{2029}paragraph",
        MetadataDictionary::new(),
    );

    assert_eq!(
        format_error_message_with_location(&error),
        r"fallback kind=3 @ path\\name\nline\rreturn\ttab\u{85}next\u{2028}line\u{2029}paragraph"
    );
}

#[test]
fn make_any_error_preserves_context() {
    struct ContextError {
        metadata: MetadataDictionary,
        context: Vec<String>,
    }

    impl ErrorFacts for ContextError {
        fn category(&self) -> &str {
            "custom"
        }

        fn kind(&self) -> i32 {
            7
        }

        fn location(&self) -> &str {
            "loc"
        }

        fn metadata(&self) -> &MetadataDictionary {
            &self.metadata
        }

        fn context(&self) -> &[String] {
            &self.context
        }
    }

    let source = ContextError {
        metadata: MetadataDictionary::new(),
        context: vec![String::from("outer"), String::from("inner")],
    };
    assert_eq!(make_any_error(&source).context(), ["outer", "inner"]);
}

// --- fail ---

#[test]
fn fail_converts_a_domain_error_and_stamps_the_context() {
    let metadata = BTreeMap::from([(String::from("detail"), MetadataValue::from("preserved"))]);
    let error = TestError {
        category: String::from("custom"),
        kind: 7,
        location: String::from("test.cpp > test > #1"),
        metadata: metadata.clone(),
    };

    let failed: Result<(), AnyError> = fail(&error, "bpm_order_shipped");

    let failure = failed.expect_err("fail is the failure");
    // The kind survives: it is what a dead-letter alert reads to say what broke.
    assert_eq!(failure.category(), "custom");
    assert_eq!(failure.kind(), 7);
    assert_eq!(*failure.metadata(), metadata);
    assert_eq!(failure.context(), ["bpm_order_shipped"]);
}

#[test]
fn fail_accepts_an_any_error_and_keeps_its_chain_beneath() {
    // An already-converted error re-homed by an outer boundary keeps the
    // context it arrived with.
    let inner = AnyError::at_location(
        AnyErrorKind::new("net", 404),
        "loc",
        MetadataDictionary::new(),
    )
    .with_context("order_lookup");

    let failed: Result<(), AnyError> = fail(&inner, "admin_ready_to_ship");

    let failure = failed.expect_err("fail is the failure");
    assert_eq!(failure.category(), "net");
    assert_eq!(failure.kind(), 404);
    assert_eq!(failure.context(), ["admin_ready_to_ship", "order_lookup"]);
}

// --- error_detail: the one read of a stated reason ---

const PROCESS_ERROR: AnyErrorKind = AnyErrorKind::new("process", 1);

fn card_declined() -> TestError {
    TestError {
        category: String::from("custom"),
        kind: 7,
        location: String::from("test.cpp > test > #1"),
        metadata: BTreeMap::from([(String::from("detail"), MetadataValue::from("card_declined"))]),
    }
}

#[test]
fn error_detail_returns_the_stated_reason() {
    let error = AnyError::from_detail(PROCESS_ERROR, "posix_spawnp failed: No such file");
    assert_eq!(
        error_detail(&error, "spawn failed"),
        "posix_spawnp failed: No such file"
    );
}

#[test]
fn error_detail_falls_back_when_no_reason_is_stated() {
    let error = AnyError::from_kind(PROCESS_ERROR);
    assert_eq!(error_detail(&error, "spawn failed"), "spawn failed");
}

#[test]
fn an_empty_detail_states_no_reason() {
    let error = AnyError::from_detail(PROCESS_ERROR, "");
    assert_eq!(error_detail(&error, "spawn failed"), "spawn failed");
}

#[test]
fn a_non_string_detail_states_no_reason() {
    // Not stringified: for a nested error that would put its source location
    // into a message a user reads.
    let nested = AnyError::from_kind(AnyErrorKind::new("net", 1));
    let metadata = BTreeMap::from([(String::from("detail"), MetadataValue::from(nested))]);
    let error = AnyError::at_location(PROCESS_ERROR, "file.cpp > func > #12", metadata);
    assert_eq!(error_detail(&error, "spawn failed"), "spawn failed");
}

#[test]
fn other_message_keys_are_not_the_detail() {
    let metadata = BTreeMap::from([(
        String::from("reason"),
        MetadataValue::from("no mesh bodies"),
    )]);
    let error = AnyError::at_location(
        AnyErrorKind::new("viewer", 1),
        "load.cpp > run > #5",
        metadata,
    );
    assert_eq!(error_detail(&error, "load failed"), "load failed");
}

#[test]
fn error_detail_reads_a_domain_error_without_a_conversion() {
    assert_eq!(
        error_detail(&card_declined(), "capture failed"),
        "card_declined"
    );
}

// --- error_detail_view: the same read, borrowed ---

#[test]
fn error_detail_view_borrows_the_reason_inside_the_errors_own_metadata() {
    let error = AnyError::from_detail(PROCESS_ERROR, "posix_spawnp failed: No such file");

    let view = error_detail_view(&error);

    assert_eq!(view, "posix_spawnp failed: No such file");
    // A view into the error, not into a copy of it.
    let stored = error.metadata()["detail"]
        .as_string()
        .expect("the detail is a string");
    assert_eq!(view.as_ptr(), stored.as_ptr());
}

#[test]
fn error_detail_view_is_empty_when_no_reason_is_stated() {
    // The predicate `error_detail` applies: absent, empty and non-string all
    // state nothing.
    let absent = AnyError::from_kind(PROCESS_ERROR);
    let empty = AnyError::from_detail(PROCESS_ERROR, "");
    let non_string = AnyError::at_location(
        PROCESS_ERROR,
        "file.cpp > func > #12",
        BTreeMap::from([(String::from("detail"), MetadataValue::from(42_i64))]),
    );

    assert_eq!(error_detail_view(&absent), "");
    assert_eq!(error_detail_view(&empty), "");
    assert_eq!(error_detail_view(&non_string), "");
}

#[test]
fn error_detail_view_reads_a_domain_error_without_a_conversion() {
    let error = card_declined();
    assert_eq!(error_detail_view(&error), "card_declined");
}

// fidelity-compile-proof: `ErrorDetailViewTest.RefusesATemporaryErrorTheViewWouldOutlive`.
// The view borrows the error it reads, so the compiler refuses one that would
// outlive it — the dangling view the source deletes an overload to prevent.
// This item pins the signature that makes it so: the returned `&str` is tied to
// the error's own lifetime.
const _: for<'error> fn(&'error AnyError) -> &'error str = error_detail_view::<AnyError>;

#[test]
fn prefixed_error_detail_joins_the_sentence_and_the_stated_reason() {
    let error = AnyError::from_detail(PROCESS_ERROR, "label already scanned");
    assert_eq!(
        prefixed_error_detail(&error, "Carrier label void rejected"),
        "Carrier label void rejected: label already scanned"
    );
}

#[test]
fn prefixed_error_detail_is_the_sentence_alone_when_no_reason_is_stated() {
    // Why this is a function and not `prefix + ": " + detail` at the call site:
    // that spelling ends in a dangling ": " whenever the reason is absent.
    let absent = AnyError::from_kind(PROCESS_ERROR);
    let empty = AnyError::from_detail(PROCESS_ERROR, "");

    assert_eq!(
        prefixed_error_detail(&absent, "Tracking lookup failed"),
        "Tracking lookup failed"
    );
    assert_eq!(
        prefixed_error_detail(&empty, "Tracking lookup failed"),
        "Tracking lookup failed"
    );
}

// Erasing an `AnyError` through its own `ErrorFacts` keeps the origin it
// recorded — `make_any_error` reads the location through the trait.
#[test]
fn make_any_error_of_an_any_error_keeps_its_location() {
    let error = AnyError::at_location(
        AnyErrorKind::new("net", 404),
        "client.rs > #12:5",
        MetadataDictionary::new(),
    );
    assert_eq!(make_any_error(&error).location(), "client.rs > #12:5");
}
