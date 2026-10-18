//! The named kinds this crate's structured errors carry into `bc-error-rs`.
//!
//! **Two categories, deliberately.** `AnyError`'s identity is the pair
//! (category, value), and its value is a bare integer *within* a category — so
//! two taxonomies sharing a category must never share a number. This crate has
//! two taxonomies:
//!
//! - [`JsonValidationErrorKind`](crate::JsonValidationErrorKind) crosses under
//!   category `"json"` at its own `#[repr(i32)]` discriminants, which are a
//!   committed wire value and are not this module's to renumber.
//! - Everything else — the strict read tier, and the merge/emit refusals —
//!   crosses under [`CATEGORY`] below.
//!
//! Holding them apart by *category* rather than by disjoint number blocks is
//! what makes a collision unwritable instead of merely watched for. The
//! alternative was one category with a reserved range and a guard over it, and
//! a guard is the weaker answer: `JsonValidationErrorKind` is
//! `#[non_exhaustive]` and its own documentation tells the next author to take
//! the next number, so a single shared space is a collision the crate
//! *instructs* someone to create. What a guard cannot say, a namespace can.

use bc_error::AnyErrorKind;

/// The category for this crate's structured errors other than validation.
///
/// The source library's own spelling for the same set. Distinct from the
/// `"json"` that [`JsonValidationErrorKind`](crate::JsonValidationErrorKind)
/// crosses under — see this module's own documentation for why that separation
/// is the design and not an accident.
pub const CATEGORY: &str = "bc::json";

/// A required key was absent. The source library's `missing_field`.
pub const MISSING_FIELD: AnyErrorKind = AnyErrorKind::new(CATEGORY, 1);

/// A key, or an element of one, was present holding another type. The source
/// library's `type_mismatch`.
pub const TYPE_MISMATCH: AnyErrorKind = AnyErrorKind::new(CATEGORY, 2);

/// A key was present holding `null` where `null` was refused. The source
/// library's `invalid_value`.
pub const INVALID_VALUE: AnyErrorKind = AnyErrorKind::new(CATEGORY, 3);

/// A write refused after its inputs had already been validated.
///
/// Numbered outside the source library's set, which stops at its
/// `parse_failed = 4`, because it names a failure that set has no member for:
/// the source's kinds describe reading, and this one describes emitting. The
/// source's `parse_failed` itself is not ported — the only malformed-input path
/// in this crate is [`JsonValidationError`](crate::JsonValidationError), which
/// already crosses under its own category with its own byte offset, and a
/// second spelling of it would be two answers to one question.
pub const EMIT_FAILED: AnyErrorKind = AnyErrorKind::new(CATEGORY, 11);
