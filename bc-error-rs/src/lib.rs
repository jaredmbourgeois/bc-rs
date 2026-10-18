//! Structured, recursively annotated error values for the bc ecosystem.
//!
//! The crate pairs [`AnyError`] with deterministic [`MetadataValue`] trees and a
//! single extension trait for Rust's native [`Result`].
//!
//! ```
//! use bc_error::{AnyError, AnyErrorKind};
//!
//! const MISSING_TABLE: AnyErrorKind = AnyErrorKind::new("config", 2);
//!
//! let error = AnyError::from_detail(MISSING_TABLE, "missing [server] table")
//!     .with_context("loading app.toml");
//!
//! assert!(error.matches(&MISSING_TABLE));
//! assert_eq!(
//!     error.to_string(),
//!     "loading app.toml: config kind=2: missing [server] table"
//! );
//! ```
//!
//! The variant set of [`MetadataValue`] is public, so callers `match` rather
//! than probe through the `as_*` accessors:
//!
//! ```
//! use bc_error::MetadataValue;
//!
//! let described = match MetadataValue::from(7_i64) {
//!     MetadataValue::Int64(value) => format!("signed {value}"),
//!     MetadataValue::String(value) => format!("text {value}"),
//!     _ => "something else".to_owned(),
//! };
//!
//! assert_eq!(described, "signed 7");
//! ```

// The rest of the lint policy lives in Cargo.toml's [lints] table so the IDE
// reads it too; this one stays here because [lints] would also apply it to the
// integration-test crates, which have no crate docs to write.
#![deny(missing_docs)]

// Must sit at the crate root: the derives on the types below expand to
// references to a `crate::UniFfiTag` it defines. The binding itself is in `ffi`.
#[cfg(feature = "uniffi")]
uniffi::setup_scaffolding!();

mod any_error_kind;
mod domain_error;
mod error;
pub mod error_facts;
#[cfg(feature = "uniffi")]
pub mod ffi;
mod first_failure;
mod location;
mod metadata_value;
mod result;

pub use any_error_kind::{AnyErrorKind, ErrorCondition, RefusalKind};
pub use domain_error::{CarriesFacts, DomainError, DomainErrorBase};
pub use error::{
    AnyError, ErrorFacts, MESSAGE_METADATA_KEYS, error_detail, error_detail_view, fail,
    format_error_message, format_error_message_with_location, make_any_error,
    prefixed_error_detail,
};
pub use error_facts::{
    caused_by, is_commit_outcome_unknown, is_corrupt_row, is_invariant_breach,
    is_permanent_failure, is_refusal, sqlstate_of, with_commit_unknown_fact, with_corrupt_row_fact,
    with_facts_of, with_invariant_breach_fact, with_refusal_fact, with_sqlstate_fact,
};
pub use first_failure::FirstFailure;
pub use location::{
    LOCATION_SEPARATOR, format_location, format_location_from, format_location_from_with_separator,
    format_location_with_separator,
};
pub use metadata_value::{
    MetadataArray, MetadataDictionary, MetadataDictionaryExt, MetadataType, MetadataValue,
};
pub use result::ResultOptionalExt;

// Compiles every ```rust block in the README as a doctest, so a stale example
// fails CI instead of shipping. Not the crate docs — only doctest collection
// sees this item.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeExamples;
