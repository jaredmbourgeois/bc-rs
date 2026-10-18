//! Five facts about a failure that a caller above the store still has to be
//! able to ask about, after the store's error has been folded into a domain
//! type's kind or erased into an [`AnyError`]:
//!
//! - **`sqlstate`** — the `PostgreSQL` SQLSTATE, so a handler can name a store
//!   fault's cause in a fixed-vocabulary attribute.
//! - **`corrupt_row`** — a row that exists and will never decode. Permanent:
//!   retrying will not help.
//! - **`commit_outcome_unknown`** — COMMIT was sent and its acknowledgment never
//!   arrived. The commit may have landed, so a caller must not compensate on it.
//! - **`invariant_breach`** — our own code broke a rule it states about what it
//!   wrote. Permanent, like a corrupt row: every retry builds the same thing.
//! - **`refused`** — the operation refused its inputs, and the same inputs are
//!   refused the same way until someone changes them. Permanent. Born with an
//!   error raised as a [`RefusalKind`](crate::RefusalKind).
//!
//! Each is a reserved key in the error's **metadata**, never a token in its
//! text: a string is only as durable as the least careful site that rebuilds
//! it. A fact is born with the error; [`make_any_error`] copies metadata
//! verbatim, so an erased error carries it for free, and code that builds a new
//! error out of an upstream one carries the facts across with
//! [`with_facts_of`] — or builds it with [`caused_by`], which does both halves.
//!
//! [`make_any_error`]: crate::make_any_error

use std::panic::Location;

use bc_core::report_assertion_failure_at;

use crate::{
    AnyError, CarriesFacts, ErrorCondition, ErrorFacts, MetadataValue, format_error_message,
    prefixed_error_detail,
};

/// The key of the SQLSTATE fact.
pub const SQLSTATE: &str = "sqlstate";
/// The key of the corrupt-row fact.
pub const CORRUPT_ROW: &str = "corrupt_row";
/// The key of the unknown-commit-outcome fact.
pub const COMMIT_OUTCOME_UNKNOWN: &str = "commit_outcome_unknown";
/// The key of the invariant-breach fact.
pub const INVARIANT_BREACH: &str = "invariant_breach";
/// The key of the refusal fact.
pub const REFUSED: &str = "refused";

/// Every fact, once: what [`with_facts_of`] carries and what telemetry names
/// under its own attribute rather than again as `error.meta.*`. A sixth fact is
/// added here.
pub const FACT_KEYS: [&str; 5] = [
    SQLSTATE,
    CORRUPT_ROW,
    COMMIT_OUTCOME_UNKNOWN,
    INVARIANT_BREACH,
    REFUSED,
];

/// Whether `key` is one of [`FACT_KEYS`].
#[must_use]
pub fn is_fact_key(key: &str) -> bool {
    FACT_KEYS.contains(&key)
}

/// Whether `code` is shaped like a SQLSTATE: exactly five characters of
/// `[0-9A-Z]`. Checked on read, because any error can carry metadata and the
/// code is logged as a fixed-vocabulary attribute.
#[must_use]
pub const fn is_sqlstate_shaped(code: &str) -> bool {
    let bytes = code.as_bytes();
    if bytes.len() != 5 {
        return false;
    }
    let mut index = 0;
    while index < bytes.len() {
        if !bytes[index].is_ascii_digit() && !bytes[index].is_ascii_uppercase() {
            return false;
        }
        index += 1;
    }
    true
}

/// Whether `key` holds the boolean `true` — and only that: a string `"true"`
/// is not the fact.
fn flag_is_set(error: &(impl ErrorFacts + ?Sized), key: &str) -> bool {
    matches!(
        error.metadata().get(key),
        Some(MetadataValue::Boolean(true))
    )
}

// --- Readers ---

/// Does this error describe a row that exists and cannot be decoded, rather than
/// a store that could not be reached?
#[must_use]
pub fn is_corrupt_row(error: &(impl ErrorFacts + ?Sized)) -> bool {
    flag_is_set(error, CORRUPT_ROW)
}

/// Did this failure leave a commit's outcome unknown? An error built by hand
/// carries no fact and reads `false`: the fact only ever adds caution.
#[must_use]
pub fn is_commit_outcome_unknown(error: &(impl ErrorFacts + ?Sized)) -> bool {
    flag_is_set(error, COMMIT_OUTCOME_UNKNOWN)
}

/// Did our own code refuse what it wrote?
#[must_use]
pub fn is_invariant_breach(error: &(impl ErrorFacts + ?Sized)) -> bool {
    flag_is_set(error, INVARIANT_BREACH)
}

/// Did the operation refuse its inputs for good?
#[must_use]
pub fn is_refusal(error: &(impl ErrorFacts + ?Sized)) -> bool {
    flag_is_set(error, REFUSED)
}

/// Will the same attempt fail the same way every time — a row that will never
/// decode, our own code refusing what it wrote, or a refused input? Each is
/// cured only by someone changing something, so retrying it is waiting with
/// extra steps. A store that could not answer or an unknown commit outcome
/// answers no.
#[must_use]
pub fn is_permanent_failure(error: &(impl ErrorFacts + ?Sized)) -> bool {
    is_corrupt_row(error) || is_invariant_breach(error) || is_refusal(error)
}

/// The SQLSTATE this failure carries, if it carries a well-formed one.
#[must_use]
pub fn sqlstate_of<E: ErrorFacts + ?Sized>(error: &E) -> Option<&str> {
    match error.metadata().get(SQLSTATE) {
        Some(MetadataValue::String(code)) if is_sqlstate_shaped(code) => Some(code),
        _ => None,
    }
}

// --- Writers ---

/// `error`, marked as describing a row that will never decode.
#[must_use]
pub fn with_corrupt_row_fact<E: CarriesFacts>(error: E) -> E {
    error.with_entry(CORRUPT_ROW.to_owned(), MetadataValue::Boolean(true))
}

/// `error`, marked as leaving a commit's outcome unknown.
#[must_use]
pub fn with_commit_unknown_fact<E: CarriesFacts>(error: E) -> E {
    error.with_entry(
        COMMIT_OUTCOME_UNKNOWN.to_owned(),
        MetadataValue::Boolean(true),
    )
}

/// `error`, marked as our own code breaking a rule it states — and the breach
/// reported to the process's assertion handler, naming the caller's line: the
/// one class of error a debug build may want to stop on where it was noticed,
/// rather than three layers up. The report changes nothing about the error and
/// does nothing with no handler installed. Carrying the fact onward reports
/// nothing again: the breach was reported once, where it was found.
#[must_use]
#[track_caller]
pub fn with_invariant_breach_fact(error: AnyError) -> AnyError {
    report_assertion_failure_at(
        "our own code keeps the rule it states",
        &format_error_message(&error),
        Location::caller(),
    );
    error.with_metadata(INVARIANT_BREACH, MetadataValue::Boolean(true))
}

/// `error`, marked as a refusal: for a site that decides a refusal by
/// circumstance rather than by its condition's declaration. An error raised as
/// a [`RefusalKind`](crate::RefusalKind) needs none.
#[must_use]
pub fn with_refusal_fact<E: CarriesFacts>(error: E) -> E {
    error.with_entry(REFUSED.to_owned(), MetadataValue::Boolean(true))
}

/// `error`, carrying `code` as its SQLSTATE — unless `code` is not
/// SQLSTATE-shaped, when nothing is set: [`sqlstate_of`] would refuse to read
/// it back, and a fact that cannot be read is a lie waiting in the metadata.
#[must_use]
pub fn with_sqlstate_fact<E: CarriesFacts>(error: E, code: &str) -> E {
    if !is_sqlstate_shaped(code) {
        return error;
    }
    error.with_entry(SQLSTATE.to_owned(), MetadataValue::from(code))
}

// --- Carrying the facts onto a new error ---

/// `error`, carrying every fact `upstream` carries. A fact `upstream` does not
/// carry is left as `error` had it. The one spelling of "this error is built
/// out of that one" — the site a fact used to be lost at.
#[must_use]
pub fn with_facts_of<E: CarriesFacts>(upstream: &(impl ErrorFacts + ?Sized), error: E) -> E {
    let mut carried = error;
    for key in FACT_KEYS {
        if let Some(value) = upstream.metadata().get(key) {
            carried = carried.with_entry(key.to_owned(), value.clone());
        }
    }
    carried
}

/// An error of `kind` caused by `cause`: carrying every fact `cause` carries
/// and stating `"<what>: <cause's stated reason>"`, located at the caller. A
/// refusal condition's error is born refused and keeps the cause's facts beside
/// that.
///
/// A domain error caused by `cause` is the same two halves, written out so its
/// own constructor is called at the caller's line:
/// `with_facts_of(&cause, StoreError::new(kind, prefixed_error_detail(&cause, what)))`.
/// A helper taking the constructor as a function would locate the error inside
/// the helper when handed a `#[track_caller]` constructor directly.
#[must_use]
#[track_caller]
pub fn caused_by(
    cause: &(impl ErrorFacts + ?Sized),
    kind: impl ErrorCondition,
    what: &str,
) -> AnyError {
    with_facts_of(
        cause,
        AnyError::from_detail(kind, prefixed_error_detail(cause, what)),
    )
}
