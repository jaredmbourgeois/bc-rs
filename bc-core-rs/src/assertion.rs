//! The process's assertion handler: what it does about a violated expectation.
//!
//! A violated expectation is a rule this process's own code states and did not
//! keep — not a failed operation, which is a `Result` a caller handles, but the
//! class nothing handles, because nothing was meant to be able to happen: a
//! telemetry call that needs a span made where none is active, an error raised
//! with the `invariant_breach` fact.
//!
//! There is one handler per process, in one slot, as `bourgeois_concern`'s
//! `assertion.hpp` has it. It is ambient diagnostics, like the telemetry
//! pipeline above it: installed once, by the composition root, and in a test by
//! a scoped capture that restores what it displaced. A library that installs one
//! to change how its own reports are treated is deciding for the whole process.

use std::panic::{self, AssertUnwindSafe, Location};
use std::sync::{Arc, Mutex, PoisonError};

/// One violated expectation, as the reporting call describes it. The two texts
/// borrow from that call: a handler that keeps a failure copies them.
///
/// `detail` is for a person at a debugger or reading an abort message, and it
/// is **not** safe to export. Reporters differ in what they put there:
/// telemetry passes only a name its caller's code chose, while an invariant
/// breach passes the whole text of the error, which can hold a path or a value.
/// A handler that sends a failure beyond the process's own console sends
/// `expectation` and `location`, and leaves `detail` behind.
#[derive(Debug, Clone, Copy)]
pub struct AssertionFailure<'report> {
    /// What was expected to hold: "a span is active".
    pub expectation: &'report str,
    /// What broke it, when the reporter can say; may be empty.
    pub detail: &'report str,
    /// Where it was reported.
    pub location: &'static Location<'static>,
}

/// The stored handler — a private alias: the seam is the nominal
/// [`AssertionHandler`].
type HandlerFn = dyn Fn(&AssertionFailure<'_>) + Send + Sync;

/// What the process does about a violated expectation.
///
/// A nominal struct rather than a bare closure alias, so it can carry `Debug`
/// and grow a method without breaking a call site.
#[derive(Clone)]
pub struct AssertionHandler(Arc<HandlerFn>);

impl AssertionHandler {
    /// A handler that runs `handle` for every report.
    #[must_use]
    pub fn new(handle: impl Fn(&AssertionFailure<'_>) + Send + Sync + 'static) -> Self {
        Self(Arc::new(handle))
    }
}

impl std::fmt::Debug for AssertionHandler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AssertionHandler(..)")
    }
}

/// The process's one slot. A handler is cloned out of it under the lock and
/// called after the lock is released, so a handler that itself reports, or
/// that installs another handler, cannot deadlock on it.
static HANDLER: Mutex<Option<AssertionHandler>> = Mutex::new(None);

fn slot() -> std::sync::MutexGuard<'static, Option<AssertionHandler>> {
    // A panic cannot leave the slot half-written: it holds one value, replaced
    // whole.
    HANDLER.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Installs `handler` as the process's assertion handler — or, with `None`,
/// leaves none — and returns the one it replaced, so a scope can restore it.
///
/// A process starts with none: a report is then recorded by whoever counts it
/// (telemetry's drop counter, an error's own fact) and by nothing here.
#[must_use = "the replaced handler is how a scope restores what it displaced"]
pub fn set_assertion_handler(handler: Option<AssertionHandler>) -> Option<AssertionHandler> {
    std::mem::replace(&mut *slot(), handler)
}

/// Reports a violated expectation to the installed handler, if there is one,
/// naming the caller's location.
///
/// It never fails the code that reports: the reporters are exactly the paths
/// that must not fail their callers, so a handler that panics is contained
/// here. A handler that aborts still aborts — that is a decision about the
/// process, not a failure of the call.
#[track_caller]
pub fn report_assertion_failure(expectation: &str, detail: &str) {
    report_assertion_failure_at(expectation, detail, Location::caller());
}

/// [`report_assertion_failure`], naming a location the reporter carried rather
/// than its own caller's — a guard that reports from its destructor names where
/// it was made.
pub fn report_assertion_failure_at(
    expectation: &str,
    detail: &str,
    location: &'static Location<'static>,
) {
    let handler = slot().clone();
    let Some(handler) = handler else {
        return;
    };
    let failure = AssertionFailure {
        expectation,
        detail,
        location,
    };
    if panic::catch_unwind(AssertUnwindSafe(|| (handler.0)(&failure))).is_err() {
        // The handler panicked: the composition root's code failing. Contained,
        // because it must not reach the code that noticed the violation, which
        // has no caller of its own to tell.
    }
}

/// A handler that aborts the process on any report, in every build.
///
/// There is deliberately no handler that aborts in a debug build only: the
/// build-mode decision belongs to the one place that owns the process, which
/// installs this one from its own debug configuration.
#[must_use]
pub fn aborting_assertion_handler() -> AssertionHandler {
    AssertionHandler::new(|_failure| std::process::abort())
}
