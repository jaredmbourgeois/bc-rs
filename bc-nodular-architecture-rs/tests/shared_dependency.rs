use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use bc_date_time::{Epoch, EpochSource, fixed_epoch_source};
use bc_nodular_architecture::{AppIdentifier, SharedDependency};

fn fixed_dependency() -> SharedDependency {
    SharedDependency::new(fixed_epoch_source(Epoch::ZERO))
}

#[test]
fn shared_dependency_new_has_pure_defaults() {
    let dependency = fixed_dependency();

    assert_eq!(dependency.app_identifier.as_str(), "");
    assert!(
        dependency.arguments.is_empty(),
        "{:?}",
        dependency.arguments
    );
    assert!(dependency.environment.is_empty());
    assert!(dependency.stringify_encoding.is_none());
    assert!(dependency.stringify_sorting_by.is_none());
}

#[test]
fn shared_dependency_uses_the_injected_epoch_source() {
    let expected = Epoch::from_millis(3_600_000);
    let dependency = SharedDependency::new(fixed_epoch_source(expected));

    assert_eq!(dependency.epoch_source.now(), expected);
}

#[test]
fn shared_dependency_default_event_sequence_increments() {
    let dependency = fixed_dependency();

    assert_eq!(
        [
            dependency.next_event_sequence(),
            dependency.next_event_sequence(),
            dependency.next_event_sequence(),
        ],
        [1, 2, 3]
    );
}

#[test]
fn shared_dependency_custom_string_encoding_is_callable() {
    let dependency = fixed_dependency().with_stringify_encoding(str::to_uppercase);

    assert_eq!(
        dependency
            .stringify_encoding
            .as_ref()
            .unwrap()
            .encode("mixed"),
        "MIXED"
    );
}

#[test]
fn shared_dependency_custom_string_sorting_is_callable() {
    let dependency = fixed_dependency().with_stringify_sorting_by(|left, right| left > right);
    let sorting = dependency.stringify_sorting_by.as_ref().unwrap();

    assert!(sorting.sorts_before("z", "a"));
    assert!(!sorting.sorts_before("a", "z"));
}

#[test]
fn shared_dependency_construction_does_not_read_the_epoch_source() {
    let reads = Arc::new(AtomicU64::new(0));
    let callback_reads = Arc::clone(&reads);
    let epoch_source = EpochSource::new(move || {
        callback_reads.fetch_add(1, Ordering::Relaxed);
        Epoch::ZERO
    });

    let _dependency = SharedDependency::new(epoch_source);

    assert_eq!(reads.load(Ordering::Relaxed), 0);
}

#[test]
fn shared_dependency_shared_returns_self() {
    let mut dependency = fixed_dependency();
    dependency.app_identifier = AppIdentifier::new("test_app");

    assert!(std::ptr::eq(
        dependency.shared(),
        std::ptr::from_ref(&dependency)
    ));
    assert_eq!(dependency.shared().app_identifier.as_str(), "test_app");
}

#[test]
fn shared_dependency_injected_epoch_source_is_read_each_time() {
    let millis = Arc::new(AtomicI64::new(100));
    let callback_millis = Arc::clone(&millis);
    let dependency = SharedDependency::new(EpochSource::new(move || {
        Epoch::from_millis(callback_millis.fetch_add(1, Ordering::Relaxed))
    }));

    assert_eq!(dependency.epoch_source.now(), Epoch::from_millis(100));
    assert_eq!(dependency.epoch_source.now(), Epoch::from_millis(101));
}

#[test]
fn shared_dependency_event_sequence_is_injectable() {
    let counter = Arc::new(AtomicU64::new(100));
    let callback_counter = Arc::clone(&counter);
    let dependency = fixed_dependency()
        .with_event_sequence_source(move || callback_counter.fetch_add(1, Ordering::Relaxed) + 1);

    assert_eq!(dependency.next_event_sequence(), 101);
    assert_eq!(dependency.next_event_sequence(), 102);
}

#[test]
fn shared_dependency_clone_retains_injected_event_sequence_behavior() {
    let counter = Arc::new(AtomicU64::new(0));
    let callback_counter = Arc::clone(&counter);
    let dependency = fixed_dependency()
        .with_event_sequence_source(move || callback_counter.fetch_add(1, Ordering::Relaxed) + 1);
    let cloned = dependency.clone();

    assert_eq!(dependency.next_event_sequence(), 1);
    assert_eq!(cloned.next_event_sequence(), 2);
}

#[test]
fn shared_dependency_accepts_a_custom_app_identifier_without_validation() {
    let mut dependency = fixed_dependency();
    dependency.app_identifier = AppIdentifier::new("com bourgeois/test");

    assert_eq!(dependency.app_identifier.as_str(), "com bourgeois/test");
}

#[test]
fn shared_dependency_accepts_custom_arguments() {
    let mut dependency = fixed_dependency();
    dependency.arguments = vec!["--verbose".to_owned(), "--port=8080".to_owned()];

    assert_eq!(dependency.arguments, ["--verbose", "--port=8080"]);
}

#[test]
fn shared_dependency_accepts_a_custom_environment() {
    let mut dependency = fixed_dependency();
    dependency.environment = BTreeMap::from([
        ("HOME".to_owned(), "/tmp".to_owned()),
        ("PATH".to_owned(), "/usr/bin".to_owned()),
    ]);

    assert_eq!(dependency.environment.len(), 2);
    assert_eq!(dependency.environment.get("HOME").unwrap(), "/tmp");
}
