//! The assertion handler, against `libs/foundation/core/test/assertion_test.cpp`.
//!
//! Each test is named for the property it asserts rather than for the C++ case
//! it came from. The tests account for each of the original's 9 cases.
//!
//! The handler is process-wide, and the default test harness runs cases on
//! threads of one process, so every case holds `ONE_HANDLER_AT_A_TIME` — the
//! invariant itself, not a guess at timing. Each installs what it needs through
//! a capture that restores the handler it displaced.

use std::panic::Location;
use std::process::Command;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use bc_core::{
    AssertionFailure, AssertionHandler, aborting_assertion_handler, report_assertion_failure,
    report_assertion_failure_at, set_assertion_handler,
};

static ONE_HANDLER_AT_A_TIME: Mutex<()> = Mutex::new(());

fn one_handler_at_a_time() -> MutexGuard<'static, ()> {
    ONE_HANDLER_AT_A_TIME
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// One reported failure, copied out of what the handler only borrows.
#[derive(Debug, Clone, PartialEq)]
struct Captured {
    expectation: String,
    detail: String,
    location: &'static Location<'static>,
}

/// Captures every failure reported while it lives, and restores the handler it
/// displaced when it goes.
struct Capture {
    captured: Arc<Mutex<Vec<Captured>>>,
    /// What was installed before, `None` meaning no handler.
    previous: Option<AssertionHandler>,
}

impl Capture {
    fn install() -> Self {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&captured);
        let previous = set_assertion_handler(Some(AssertionHandler::new(
            move |failure: &AssertionFailure<'_>| {
                sink.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(Captured {
                        expectation: failure.expectation.to_string(),
                        detail: failure.detail.to_string(),
                        location: failure.location,
                    });
            },
        )));
        Self { captured, previous }
    }

    fn failures(&self) -> Vec<Captured> {
        self.captured
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        // The handler this one displaced comes back; this capture's own is what
        // the replacement hands over, and it is done with.
        drop(set_assertion_handler(self.previous.take()));
    }
}

#[test]
fn a_report_with_no_handler_installed_does_nothing() {
    let _one = one_handler_at_a_time();
    let previous = set_assertion_handler(None);
    report_assertion_failure("nothing listens", "");
    drop(set_assertion_handler(previous));
}

#[test]
fn the_installed_handler_receives_what_was_reported_and_where() {
    let _one = one_handler_at_a_time();
    let capture = Capture::install();
    let here = Location::caller();
    report_assertion_failure("a span is active", "add_span_event(\"orphan\")");
    let failures = capture.failures();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].expectation, "a span is active");
    assert_eq!(failures[0].detail, "add_span_event(\"orphan\")");
    // The location is the reporting call's own line, one below `here`.
    assert_eq!(failures[0].location.file(), here.file());
    assert_eq!(failures[0].location.line(), here.line() + 1);
}

#[test]
fn a_detail_may_be_empty_and_a_location_may_be_handed_in() {
    let _one = one_handler_at_a_time();
    let capture = Capture::install();
    let carried = Location::caller();
    report_assertion_failure_at("a span guard is consumed on its thread", "", carried);
    let failures = capture.failures();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].detail, "");
    assert_eq!(failures[0].location, carried);
}

#[test]
fn installing_a_handler_returns_the_one_it_replaced() {
    let _one = one_handler_at_a_time();
    let outer = Capture::install();
    let inner = Capture::install();
    report_assertion_failure("seen by the inner capture", "");
    drop(inner);
    report_assertion_failure("seen by the outer capture", "");
    let failures = outer.failures();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].expectation, "seen by the outer capture");
}

#[test]
fn installing_none_leaves_none_and_returns_none_when_none_was_there() {
    let _one = one_handler_at_a_time();
    let previous = set_assertion_handler(None);
    assert!(set_assertion_handler(None).is_none());
    drop(set_assertion_handler(previous));
}

#[test]
fn a_handler_that_panics_does_not_fail_the_code_that_reported() {
    let _one = one_handler_at_a_time();
    let previous = set_assertion_handler(Some(AssertionHandler::new(|_failure| {
        panic!("the handler is the composition root's code, and it failed");
    })));
    report_assertion_failure("contained", "");
    drop(set_assertion_handler(previous));
}

#[test]
fn a_handler_may_report_and_reinstall_without_deadlocking() {
    let _one = one_handler_at_a_time();
    let reports = Arc::new(Mutex::new(0_u32));
    let counter = Arc::clone(&reports);
    let previous = set_assertion_handler(Some(AssertionHandler::new(move |failure| {
        *counter.lock().unwrap_or_else(PoisonError::into_inner) += 1;
        // Both reach the slot the report itself read; neither may wait on it.
        if failure.expectation == "outer" {
            report_assertion_failure("inner", "");
            drop(set_assertion_handler(None));
        }
    })));
    let count = || *reports.lock().unwrap_or_else(PoisonError::into_inner);

    report_assertion_failure("outer", "");
    assert_eq!(count(), 2);
    // The handler removed itself, so nothing is installed now.
    report_assertion_failure("after removal", "");
    assert_eq!(count(), 2);
    drop(set_assertion_handler(previous));
}

#[test]
fn a_capture_restores_the_handler_it_displaced() {
    let _one = one_handler_at_a_time();
    let outer = Capture::install();
    {
        let inner = Capture::install();
        report_assertion_failure("seen by the inner capture", "");
        assert_eq!(inner.failures().len(), 1);
        assert!(outer.failures().is_empty(), "{:?}", outer.failures());
    }
    report_assertion_failure("seen by the outer capture", "");
    assert_eq!(outer.failures().len(), 1);
}

/// The child the death test below runs in a process of its own: it installs
/// the aborting handler and reports. Ignored, so it runs only when named.
#[test]
#[ignore = "aborts its process by design; run by the_aborting_handler_aborts_in_every_build"]
fn the_aborting_handler_child() {
    drop(set_assertion_handler(Some(aborting_assertion_handler())));
    report_assertion_failure("this process ends here", "");
}

#[test]
fn the_aborting_handler_aborts_in_every_build() {
    let output = Command::new(std::env::current_exe().expect("this test binary"))
        .args([
            "--exact",
            "the_aborting_handler_child",
            "--ignored",
            "--nocapture",
        ])
        .output()
        .expect("run the child");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // A child that ran nothing exits 0 too: refuse a name that matched no test.
    assert!(
        stdout.contains("running 1 test"),
        "the child ran no test:\n{stdout}"
    );
    assert!(!output.status.success(), "the child was not aborted");
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        // SIGABRT.
        assert_eq!(output.status.signal(), Some(6));
    }
}
