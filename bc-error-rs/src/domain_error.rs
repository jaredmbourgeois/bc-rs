//! The shape every domain error type shares, held once.
//!
//! A domain error — a payment failure, a store failure — is its own type with
//! its own kinds, and every one of them carries the same three things: a kind,
//! where it was raised, and metadata. The source holds those in a base class;
//! here a type holds a [`DomainErrorBase`] and says so by implementing
//! [`DomainError`], which gives it [`ErrorFacts`] and somewhere for a fact about
//! the failure to be attached ([`CarriesFacts`]).
//!
//! Equality stays the type's own: the base offers
//! [`same_kind_and_location`](DomainErrorBase::same_kind_and_location), what
//! every domain type's `==` compares, and derives no `PartialEq` of its own — a
//! shared one would make two different error types comparable.

use crate::{AnyError, ErrorFacts, MetadataDictionary, MetadataValue, format_location};

/// A domain error's kind, location and metadata.
#[derive(Clone, Debug)]
pub struct DomainErrorBase {
    kind: i32,
    location: Box<str>,
    metadata: MetadataDictionary,
}

impl DomainErrorBase {
    /// A base seeding `kind_name` and nothing else, located at its caller.
    ///
    /// The domain type's own constructor is `#[track_caller]` too, so the
    /// location is where that constructor was called — not where it calls this.
    #[must_use]
    #[track_caller]
    pub fn new(kind: i32, kind_name: &str) -> Self {
        let mut metadata = MetadataDictionary::new();
        metadata.insert("kind_name".to_owned(), MetadataValue::from(kind_name));
        Self {
            kind,
            location: format_location().into_boxed_str(),
            metadata,
        }
    }

    /// A base seeding `kind_name` and the stated reason, `detail`.
    #[must_use]
    #[track_caller]
    pub fn with_detail(kind: i32, kind_name: &str, detail: impl Into<String>) -> Self {
        let mut base = Self::new(kind, kind_name);
        base.set_metadata("detail", MetadataValue::from(detail.into()));
        base
    }

    /// A base holding `metadata` verbatim and seeding nothing: for a type whose
    /// constructor takes a metadata map of its own.
    #[must_use]
    #[track_caller]
    pub fn from_metadata(kind: i32, metadata: MetadataDictionary) -> Self {
        Self {
            kind,
            location: format_location().into_boxed_str(),
            metadata,
        }
    }

    /// The kind, as the type's own enum stored it.
    #[must_use]
    pub const fn kind(&self) -> i32 {
        self.kind
    }

    /// Where the error was raised.
    #[must_use]
    pub fn location(&self) -> &str {
        &self.location
    }

    /// The error's metadata.
    #[must_use]
    pub const fn metadata(&self) -> &MetadataDictionary {
        &self.metadata
    }

    /// Adds or replaces one metadata entry. Keys follow the `OTel` dot
    /// convention (`order.id`), except the reserved fact keys
    /// [`error_facts`](crate::error_facts) owns.
    pub fn set_metadata(&mut self, key: impl Into<String>, value: MetadataValue) {
        self.metadata.insert(key.into(), value);
    }

    /// What every domain type's `==` compares: the kind and the location, never
    /// the detail.
    #[must_use]
    pub fn same_kind_and_location(&self, other: &Self) -> bool {
        self.kind == other.kind && self.location == other.location
    }
}

/// An error type built on a [`DomainErrorBase`]: its category, and its base.
///
/// Implementing it gives the type [`ErrorFacts`] — so [`make_any_error`]
/// erases it and every fact reader reads it — and [`CarriesFacts`].
///
/// ```
/// use bc_error::{DomainError, DomainErrorBase, ErrorFacts, error_detail};
///
/// struct PaymentError {
///     base: DomainErrorBase,
/// }
///
/// impl PaymentError {
///     #[track_caller]
///     fn declined(detail: &str) -> Self {
///         Self { base: DomainErrorBase::with_detail(1, "declined", detail) }
///     }
/// }
///
/// impl DomainError for PaymentError {
///     const CATEGORY: &'static str = "payment";
///     fn base(&self) -> &DomainErrorBase {
///         &self.base
///     }
///     fn base_mut(&mut self) -> &mut DomainErrorBase {
///         &mut self.base
///     }
/// }
///
/// let error = PaymentError::declined("card_declined");
/// assert_eq!(error.category(), "payment");
/// assert_eq!(error_detail(&error, "capture failed"), "card_declined");
/// ```
///
/// [`make_any_error`]: crate::make_any_error
pub trait DomainError {
    /// The category every error of this type carries.
    const CATEGORY: &'static str;

    /// The error's base.
    fn base(&self) -> &DomainErrorBase;

    /// The error's base, to add metadata to.
    fn base_mut(&mut self) -> &mut DomainErrorBase;
}

impl<E: DomainError> ErrorFacts for E {
    fn category(&self) -> &str {
        E::CATEGORY
    }

    fn kind(&self) -> i32 {
        self.base().kind()
    }

    fn location(&self) -> &str {
        self.base().location()
    }

    fn metadata(&self) -> &MetadataDictionary {
        self.base().metadata()
    }
}

/// An error a fact can be attached to: an [`AnyError`], or any
/// [`DomainError`]. What the fact writers in [`error_facts`](crate::error_facts)
/// take, so one writer serves both, where the source needs one form for a
/// mutable domain error and another for an immutable `AnyError`.
pub trait CarriesFacts: ErrorFacts + Sized {
    /// This error, with `key` set to `value` in its metadata.
    #[must_use]
    fn with_entry(self, key: String, value: MetadataValue) -> Self;
}

impl CarriesFacts for AnyError {
    fn with_entry(self, key: String, value: MetadataValue) -> Self {
        self.with_metadata(key, value)
    }
}

impl<E: DomainError> CarriesFacts for E {
    fn with_entry(mut self, key: String, value: MetadataValue) -> Self {
        self.base_mut().set_metadata(key, value);
        self
    }
}
