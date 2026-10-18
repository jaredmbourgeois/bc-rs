//! Ported from `libs/foundation/error/test/first_failure_test.cpp`.

use bc_error::{AnyError, AnyErrorKind, FirstFailure, MetadataValue};

const ITEM_FAILED: AnyErrorKind = AnyErrorKind::new("batch", 1);

fn failure(item: &str) -> AnyError {
    AnyError::from_kind(ITEM_FAILED).with_context(item)
}

#[test]
fn a_batch_with_no_failure_takes_nothing() {
    let failures = FirstFailure::new();
    assert_eq!(failures.count(), 0);
    assert_eq!(failures.take(), None);
}

#[test]
fn the_first_failure_is_reported_and_every_failure_is_counted() {
    let mut failures = FirstFailure::new();
    failures.keep(failure("first_item"));
    failures.keep(failure("second_item"));
    failures.keep(failure("third_item"));
    assert_eq!(failures.count(), 3);

    let taken = failures.take().expect("a failure was kept");
    assert_eq!(taken.context(), ["first_item"]);
    assert_eq!(
        taken.metadata().get("batch.failures"),
        Some(&MetadataValue::Int64(3))
    );
}
