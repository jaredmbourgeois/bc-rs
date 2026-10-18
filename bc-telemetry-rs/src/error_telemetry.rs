//! Errors as telemetry: the port of `libs/platform/error_telemetry`'s
//! `record_error_telemetry.hpp`.
//!
//! Three verbs, and the lines between them are the C++ original's:
//!
//! - [`record_error_telemetry`] is for every error the code meets: an `error`
//!   event on the current span, and the span's status set to error. Where no
//!   span is current it records nothing, and that is the ordinary case rather
//!   than a misuse — an error is recorded wherever it is met.
//! - [`report_error`] is for an error the code could not answer past: it
//!   records, then writes an error-level log record of the error's identity.
//! - [`report_degraded`] is for an error the code answered past with a lesser
//!   result: the same, at warn level.
//!
//! All three work on the span current on this thread — the event on it, the log
//! under it — which is what "the request that failed" means here, not a default
//! standing in for a choice.
//!
//! A log record leaves the process carrying only the error's **identity**:
//! category, kind, location, context chain and the structured facts. Its
//! metadata is free text some store or handler composed — a detail can hold a
//! customer's email — so it rides only on the span event, read beside the
//! request that failed.

use bc_error::{
    AnyError, ErrorFacts, error_facts, is_commit_outcome_unknown, is_corrupt_row,
    is_invariant_breach, is_refusal, sqlstate_of,
};

use crate::context::{Parent, SpanStatus, add_span_event_if_any, set_span_status_if_any};
use crate::log_record::{Severity, record_log};
use crate::types::{AttributeValue, Attributes};

/// An error a generated conversion was handed that is not the declared error it
/// converts to: handed back unchanged, with why, so the caller still holds it to
/// record or return.
#[derive(Debug, Clone, PartialEq)]
pub struct NotThisError {
    /// The error, unchanged.
    pub error: AnyError,
    /// Why it is not the one asked for.
    pub reason: NotThisErrorReason,
}

/// Why an error is not the declared error a conversion asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotThisErrorReason {
    /// Of another kind: another module's error, or another of this module's.
    OtherKind,
    /// Of this kind, without a field its constructor always writes — so built
    /// by hand rather than through that constructor, and its report would drop
    /// the field.
    MissingField(&'static str),
}

/// What failed and where: the error's category, kind and location, its context
/// chain when it has one, and its structured facts — the SQLSTATE under `OTel`'s
/// `db.response.status_code`, and a boolean per flag. Deliberately not the
/// metadata: these are a fixed, shape-checked set, safe on a line that leaves
/// the process.
#[must_use]
pub fn error_identity_attributes(error: &(impl ErrorFacts + ?Sized)) -> Attributes {
    let mut attributes: Attributes = vec![
        (
            "error.category".to_owned(),
            AttributeValue::from(error.category()),
        ),
        (
            "error.kind".to_owned(),
            AttributeValue::Int64(i64::from(error.kind())),
        ),
        (
            "error.location".to_owned(),
            AttributeValue::from(error.location()),
        ),
    ];
    if !error.context().is_empty() {
        attributes.push((
            "error.context_chain".to_owned(),
            AttributeValue::from(error.context().join(" > ")),
        ));
    }
    if let Some(sqlstate) = sqlstate_of(error) {
        attributes.push((
            "db.response.status_code".to_owned(),
            AttributeValue::from(sqlstate),
        ));
    }
    let flags = [
        ("error.corrupt_row", is_corrupt_row(error)),
        (
            "error.commit_outcome_unknown",
            is_commit_outcome_unknown(error),
        ),
        ("error.invariant_breach", is_invariant_breach(error)),
        ("error.refused", is_refusal(error)),
    ];
    for (name, set) in flags {
        if set {
            attributes.push((name.to_owned(), AttributeValue::Boolean(true)));
        }
    }
    attributes
}

/// Records `error` as an `error` event on the current span — its identity, and
/// every metadata entry as `error.meta.<key>` — and sets that span's status to
/// error, with `<category>:<kind>` as its message. With no span current it
/// records nothing, and counts and reports nothing either.
pub fn record_error_telemetry(error: &(impl ErrorFacts + ?Sized)) {
    let mut attributes = error_identity_attributes(error);
    for (key, value) in error.metadata() {
        // A fact is already in the identity, under its own name; naming it again
        // as `error.meta.*` would report it twice.
        if error_facts::is_fact_key(key) {
            continue;
        }
        attributes.push((
            format!("error.meta.{key}"),
            AttributeValue::from(value.stringify()),
        ));
    }
    add_span_event_if_any("error", attributes);
    set_span_status_if_any(SpanStatus::error(format!(
        "{}:{}",
        error.category(),
        error.kind()
    )));
}

/// Reports an error the code could not answer past: records it, then writes an
/// error-level log record whose body is `message` and whose attributes are the
/// error's identity followed by `context` — what the call site knows that the
/// error does not, ids and labels only. Never for a refusal the caller can
/// correct: that is not an error of ours.
pub fn report_error(message: &str, error: &(impl ErrorFacts + ?Sized), context: Attributes) {
    report(Severity::Error, message, error, context);
}

/// Reports an error the code answered past with a lesser result: as
/// [`report_error`], at warn level, because the request still got an answer.
pub fn report_degraded(message: &str, error: &(impl ErrorFacts + ?Sized), context: Attributes) {
    report(Severity::Warn, message, error, context);
}

fn report(
    severity: Severity,
    message: &str,
    error: &(impl ErrorFacts + ?Sized),
    context: Attributes,
) {
    record_error_telemetry(error);
    let mut attributes = error_identity_attributes(error);
    attributes.extend(context);
    record_log(Parent::current(), severity, message, attributes);
}
