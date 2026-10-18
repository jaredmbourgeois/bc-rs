//! Ported from `libs/platform/telemetry/test/span_concurrency_test.cpp`: an
//! open span belongs to no thread, and a guard belongs to exactly one.

mod support;

use std::sync::Barrier;
use std::thread;

use bc_telemetry::{
    Attributes, OpenSpan, Parent, SpanGuard, SpanKind, SpanStatus, StatusCode,
    current_span_context, dropped_count, ends_ok, open_span, running_span, start_span, with_span,
};

use support::{AssertionCapture, exported, ticking_clock_from, with_in_memory_configured};

/// The pipeline every case runs under: an in-memory exporter, every span
/// sampled, and a clock that advances one millisecond per read.
fn with_concurrency(body: impl FnOnce(&std::sync::Arc<bc_telemetry::InMemoryExporter>)) {
    with_in_memory_configured(
        |builder| builder.epoch_source(ticking_clock_from(1_000)),
        body,
    );
}

/// An internal span under the current one, with no attributes.
fn start_child(name: &str) -> SpanGuard {
    start_span(
        Parent::current(),
        name,
        SpanKind::Internal,
        Attributes::new(),
    )
}

// fidelity-compile-proof: `SpanConcurrency.AGuardEndedOnAnotherThreadStillExportsItsSpanAndIsReported`,
// `SpanConcurrency.AGuardReleasedOnAnotherThreadYieldsAnEndedSpanAndIsReported` and
// `SpanConcurrency.AGuardThatOutlivesTheThreadThatStartedItStillEndsItsSpan`.
// A `SpanGuard` is not `Send`, so a guard cannot be carried to another thread
// to be ended, released or outlive its own — each of the three is a program
// the compiler refuses. The proof is the trait below: it has one impl for every
// type and a second, marked by `u8`, for every `Send` type, so naming its item
// on a `Send` type matches both and is ambiguous, which does not compile.
// `SpanGuard` matches only the first, so the test below compiles.
trait AmbiguousIfSend<Marker> {
    fn witness() {}
}
impl<T: ?Sized> AmbiguousIfSend<()> for T {}
impl<T: ?Sized + Send> AmbiguousIfSend<u8> for T {}

#[test]
fn a_span_guard_is_not_send() {
    <SpanGuard as AmbiguousIfSend<_>>::witness();
}

// fidelity-compile-proof: `SpanConcurrency.AMutatorRacingAnEndLandsOrIsReportedAndIsNeverLost`.
// Ending takes the span's handle by value, and a mutator holds it by
// reference, so a mutation and an end through one handle cannot overlap: the
// race the case runs is a program the borrow checker refuses. This item pins
// the signature that makes it so.
const _: fn(OpenSpan, SpanStatus) = OpenSpan::end;

/// What lets an open span move between threads and be shared by them: the
/// counterpart of the proof above, which says a guard can do neither.
fn is_send_and_sync<T: Send + Sync>() {}

#[test]
fn a_span_opened_on_one_thread_is_mutated_and_ended_on_another() {
    is_send_and_sync::<OpenSpan>();
    with_concurrency(|exporter| {
        let assertions = AssertionCapture::install();
        let request = open_span(
            Parent::current(),
            "request",
            SpanKind::Server,
            Attributes::new(),
        );
        let request_context = request.context();

        thread::spawn(move || {
            request.add_event("picked-up", Attributes::new());
            with_span(&request, |_request| drop(start_child("respond")));
            request.end(SpanStatus::ok());
        })
        .join()
        .expect("the worker did not panic");

        let exported_request = exported(exporter, "request");
        assert_eq!(exported_request.context, request_context);
        assert_eq!(exported_request.events.len(), 1);
        assert_eq!(exported_request.status, StatusCode::Ok);
        assert_eq!(
            exported(exporter, "respond").parent_span_id,
            Some(request_context.span_id)
        );
        // It was never current on this thread, and the worker's context went
        // with the worker.
        assert_eq!(current_span_context(), None);
        assert!(
            assertions.failures().is_empty(),
            "{:?}",
            assertions.failures()
        );
        assert_eq!(dropped_count(), 0);
    });
}

#[test]
fn concurrent_mutators_through_one_span_lose_nothing() {
    const WRITERS: usize = 4;
    const WRITES_EACH: usize = 250;
    with_concurrency(|exporter| {
        let shared = open_span(
            Parent::current(),
            "shared",
            SpanKind::Internal,
            Attributes::new(),
        );
        // Every writer waits here until all are ready, so the writes overlap.
        let start = Barrier::new(WRITERS);
        thread::scope(|scope| {
            // Each writer tags its attributes with its own number.
            for writer in (0..WRITERS).zip(0_i64..).map(|(_, number)| number) {
                let shared = &shared;
                let start = &start;
                scope.spawn(move || {
                    start.wait();
                    for _ in 0..WRITES_EACH {
                        shared.set_attribute("writer", writer);
                        shared.add_event("written", Attributes::new());
                    }
                });
            }
        });
        shared.end(SpanStatus::unset());

        let exported_span = exported(exporter, "shared");
        assert_eq!(exported_span.attributes.len(), WRITERS * WRITES_EACH);
        assert_eq!(exported_span.events.len(), WRITERS * WRITES_EACH);
    });
}

#[test]
fn one_span_may_be_current_on_two_threads_at_once() {
    with_concurrency(|exporter| {
        let assertions = AssertionCapture::install();
        let shared = open_span(
            Parent::current(),
            "shared",
            SpanKind::Internal,
            Attributes::new(),
        );
        // Each thread waits for the other inside `with_span`, so both have the
        // same span current at the same moment.
        let both_inside = Barrier::new(2);
        thread::scope(|scope| {
            for child_name in ["first-child", "second-child"] {
                let shared = &shared;
                let both_inside = &both_inside;
                scope.spawn(move || {
                    with_span(shared, |_shared| {
                        both_inside.wait();
                        drop(start_child(child_name));
                    });
                });
            }
        });
        shared.end(SpanStatus::unset());

        let shared_id = exported(exporter, "shared").context.span_id;
        assert_eq!(
            exported(exporter, "first-child").parent_span_id,
            Some(shared_id)
        );
        assert_eq!(
            exported(exporter, "second-child").parent_span_id,
            Some(shared_id)
        );
        assert!(
            assertions.failures().is_empty(),
            "{:?}",
            assertions.failures()
        );
    });
}

// Work handed to another thread carries its parent as a value taken where it
// was handed over. The worker has a span of its own current, and that is not
// the parent.
#[test]
fn a_parent_taken_where_work_is_handed_over_is_the_parent_on_the_worker() {
    with_concurrency(|exporter| {
        let request = start_child("request");
        let handed_over = Parent::current();

        thread::spawn(move || {
            let _tick = start_child("worker.tick");
            running_span(
                handed_over,
                "job.run",
                SpanKind::Internal,
                Attributes::new(),
                ends_ok,
                |_span| {},
            );
        })
        .join()
        .expect("the worker did not panic");
        drop(request);

        let job = exported(exporter, "job.run");
        let request = exported(exporter, "request");
        assert_eq!(job.parent_span_id, Some(request.context.span_id));
        assert_ne!(
            job.parent_span_id,
            Some(exported(exporter, "worker.tick").context.span_id)
        );
        assert_eq!(job.context.trace_id, request.context.trace_id);
    });
}
