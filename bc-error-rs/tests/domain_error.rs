//! Ported from `libs/foundation/error/test/domain_error_test.cpp`.

use std::collections::BTreeMap;

use bc_error::{
    DomainError, DomainErrorBase, ErrorFacts, MetadataDictionary, MetadataValue, error_detail,
    make_any_error,
};

/// The smallest real domain error: exactly the shape every production type has.
#[derive(Clone, Debug)]
struct SampleError {
    base: DomainErrorBase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SampleKind {
    First = 1,
    Second = 2,
}

impl SampleKind {
    const fn name(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Second => "second",
        }
    }

    const fn from_value(value: i32) -> Option<Self> {
        match value {
            1 => Some(Self::First),
            2 => Some(Self::Second),
            _ => None,
        }
    }
}

impl SampleError {
    #[track_caller]
    fn new(kind: SampleKind) -> Self {
        Self {
            base: DomainErrorBase::new(kind as i32, kind.name()),
        }
    }

    #[track_caller]
    fn with_detail(kind: SampleKind, detail: &str) -> Self {
        Self {
            base: DomainErrorBase::with_detail(kind as i32, kind.name(), detail),
        }
    }

    fn kind_enum(&self) -> Option<SampleKind> {
        SampleKind::from_value(self.base.kind())
    }
}

impl DomainError for SampleError {
    const CATEGORY: &'static str = "sample";
    fn base(&self) -> &DomainErrorBase {
        &self.base
    }
    fn base_mut(&mut self) -> &mut DomainErrorBase {
        &mut self.base
    }
}

impl PartialEq for SampleError {
    fn eq(&self, other: &Self) -> bool {
        self.base.same_kind_and_location(&other.base)
    }
}

/// Built from a caller-supplied metadata map.
struct MapError {
    base: DomainErrorBase,
}

impl MapError {
    #[track_caller]
    fn new(metadata: MetadataDictionary) -> Self {
        Self {
            base: DomainErrorBase::from_metadata(7, metadata),
        }
    }
}

impl DomainError for MapError {
    const CATEGORY: &'static str = "map";
    fn base(&self) -> &DomainErrorBase {
        &self.base
    }
    fn base_mut(&mut self) -> &mut DomainErrorBase {
        &mut self.base
    }
}

#[test]
fn the_kind_only_constructor_seeds_the_kind_name_and_nothing_else() {
    let error = SampleError::new(SampleKind::Second);
    assert_eq!(error.kind(), 2);
    assert_eq!(error.kind_enum(), Some(SampleKind::Second));
    assert_eq!(error.metadata().len(), 1);
    assert_eq!(error.metadata()["kind_name"].as_string(), Some("second"));
}

#[test]
fn the_detail_constructor_publishes_the_detail() {
    let error = SampleError::with_detail(SampleKind::First, "the reason");
    assert_eq!(error_detail(&error, "fallback"), "the reason");
    assert_eq!(error.metadata()["kind_name"].as_string(), Some("first"));
}

// The location must be the caller's: the domain type's constructor is
// `#[track_caller]` as the base's is, so the base reports where the type was
// built rather than its own file for every error in the tree.
#[test]
fn the_location_is_the_call_site_of_the_domain_types_constructor() {
    let error = SampleError::new(SampleKind::First);
    assert!(error.location().contains(file!()), "{}", error.location());
    assert!(
        !error.location().contains("src/domain_error.rs"),
        "{}",
        error.location()
    );
}

#[test]
fn set_metadata_adds_and_replaces() {
    let mut error = SampleError::with_detail(SampleKind::First, "before");
    error
        .base_mut()
        .set_metadata("order.id", MetadataValue::from("ord-1"));
    error
        .base_mut()
        .set_metadata("detail", MetadataValue::from("after"));
    assert_eq!(error.metadata()["order.id"].as_string(), Some("ord-1"));
    assert_eq!(error_detail(&error, ""), "after");
}

#[test]
fn a_metadata_map_is_stored_verbatim_with_no_kind_name_seeded() {
    assert!(
        MapError::new(MetadataDictionary::new())
            .metadata()
            .is_empty()
    );
    let error = MapError::new(BTreeMap::from([(
        String::from("reason"),
        MetadataValue::from("why"),
    )]));
    assert_eq!(error.metadata().len(), 1);
    assert_eq!(error.kind(), 7);
}

#[test]
fn equality_is_kind_and_location_never_the_detail() {
    // Built at one call site, so one location; the detail differs.
    let make = |kind, detail| SampleError::with_detail(kind, detail);
    assert_eq!(
        make(SampleKind::First, "one"),
        make(SampleKind::First, "two")
    );
    assert_ne!(
        make(SampleKind::First, "one"),
        make(SampleKind::Second, "one")
    );
    let built_here = SampleError::new(SampleKind::First);
    let built_a_line_later = SampleError::new(SampleKind::First);
    assert_ne!(
        built_here, built_a_line_later,
        "two call sites are two locations"
    );
}

#[test]
fn make_any_error_carries_everything_across() {
    let mut error = SampleError::with_detail(SampleKind::Second, "the reason");
    error
        .base_mut()
        .set_metadata("order.id", MetadataValue::from("ord-1"));
    let any = make_any_error(&error);
    assert_eq!(any.category(), "sample");
    assert_eq!(any.kind(), 2);
    assert_eq!(any.location(), error.location());
    assert_eq!(any.metadata(), error.metadata());
}
