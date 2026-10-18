//! A span follows its task: `with_span_async` and `running_span_async`.
//!
//! New surface, so no ported cases — the C++ original has no asynchronous
//! code to scope. Each case drives its futures by hand, poll by poll, so what is
//! current between two polls is something a case can ask; no runtime is needed,
//! which is the property under test as much as any of these.

mod support;

use std::future::Future;
use std::panic::{self, AssertUnwindSafe};
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::thread;

use bc_telemetry::{
    Attributes, InMemoryExporter, OpenSpan, Parent, SpanKind, SpanStatus, StatusCode,
    current_span_context, ends_ok, open_span, running_span_async, start_span, with_span_async,
};

use support::{exported, exported_names, ticking_clock_from, with_in_memory_configured};

fn with_tasks(body: impl FnOnce(&Arc<InMemoryExporter>)) {
    with_in_memory_configured(
        |builder| builder.epoch_source(ticking_clock_from(1_000)),
        body,
    );
}

fn open_root(name: &str) -> OpenSpan {
    open_span(Parent::none(), name, SpanKind::Internal, Attributes::new())
}

/// A future that is pending once, then ready: one `.await` that gives the
/// thread back to whatever runs next.
struct YieldOnce {
    yielded: bool,
}

fn yield_once() -> YieldOnce {
    YieldOnce { yielded: false }
}

impl Future for YieldOnce {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        if self.yielded {
            Poll::Ready(())
        } else {
            self.yielded = true;
            context.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

/// Polls `future` once with a waker that does nothing — the case decides when
/// the next poll happens.
fn poll_once<F: Future>(future: Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

#[test]
fn a_span_is_current_for_each_poll_of_its_future_and_nowhere_between() {
    with_tasks(|_| {
        let span = open_root("task");
        let context = span.context();
        let mut task = pin!(with_span_async(&span, async move {
            assert_eq!(current_span_context(), Some(context));
            yield_once().await;
            assert_eq!(current_span_context(), Some(context));
            7
        }));

        assert_eq!(poll_once(task.as_mut()), Poll::Pending);
        // Between polls the task is suspended, and its span is not current.
        assert_eq!(current_span_context(), None);
        assert_eq!(poll_once(task.as_mut()), Poll::Ready(7));
        assert_eq!(current_span_context(), None);
    });
}

// The property a server running its tasks on one thread needs: two tasks on
// one thread, interleaved at their awaits, each see their own span and never
// the other's.
#[test]
fn tasks_interleaved_on_one_thread_each_see_only_their_own_span() {
    with_tasks(|_| {
        let first = open_root("first");
        let second = open_root("second");
        let first_context = first.context();
        let second_context = second.context();
        let task = |expected| async move {
            assert_eq!(current_span_context(), Some(expected));
            yield_once().await;
            assert_eq!(current_span_context(), Some(expected));
        };
        let mut first_task = pin!(with_span_async(&first, task(first_context)));
        let mut second_task = pin!(with_span_async(&second, task(second_context)));

        assert_eq!(poll_once(first_task.as_mut()), Poll::Pending);
        assert_eq!(poll_once(second_task.as_mut()), Poll::Pending);
        assert_eq!(poll_once(first_task.as_mut()), Poll::Ready(()));
        assert_eq!(poll_once(second_task.as_mut()), Poll::Ready(()));
        assert_eq!(current_span_context(), None);
    });
}

// What a task starts belongs to the task's span, because that span is current
// while the task runs; and a task's span is not inherited by a span the thread
// starts after the poll.
#[test]
fn a_span_started_inside_a_task_is_the_tasks_child() {
    with_tasks(|exporter| {
        let span = open_root("task");
        {
            let mut task = pin!(with_span_async(&span, async {
                drop(start_span(
                    Parent::current(),
                    "inside",
                    SpanKind::Internal,
                    Attributes::new(),
                ));
                yield_once().await;
            }));
            assert_eq!(poll_once(task.as_mut()), Poll::Pending);
            drop(start_span(
                Parent::current(),
                "between",
                SpanKind::Internal,
                Attributes::new(),
            ));
            assert_eq!(poll_once(task.as_mut()), Poll::Ready(()));
        }
        span.end(SpanStatus::unset());

        let task_span_id = exported(exporter, "task").context.span_id;
        assert_eq!(
            exported(exporter, "inside").parent_span_id,
            Some(task_span_id)
        );
        assert_eq!(exported(exporter, "between").parent_span_id, None);
    });
}

// A task may be polled on one thread and resumed on another, as a
// work-stealing runtime does; its span goes with it, and stays on neither.
#[test]
fn a_task_resumed_on_another_thread_takes_its_span_with_it() {
    with_tasks(|_| {
        let span = open_root("task");
        let context = span.context();
        let mut task = Box::pin(with_span_async(&span, async move {
            yield_once().await;
            current_span_context()
        }));

        assert_eq!(poll_once(task.as_mut()), Poll::Pending);
        let seen_there = thread::scope(|scope| {
            scope
                .spawn(|| {
                    let polled = poll_once(task.as_mut());
                    (polled, current_span_context())
                })
                .join()
                .expect("the other thread did not panic")
        });

        assert_eq!(seen_there, (Poll::Ready(Some(context)), None));
        assert_eq!(current_span_context(), None);
    });
}

#[test]
fn running_span_async_starts_under_the_parent_named_and_ends_by_the_output() {
    with_tasks(|exporter| {
        let outer = start_span(
            Parent::current(),
            "outer",
            SpanKind::Internal,
            Attributes::new(),
        );
        // Named here; the future is polled after `outer` has stopped being
        // current, and its span is still `outer`'s child.
        let task = running_span_async(
            Parent::current(),
            "query",
            SpanKind::Client,
            Attributes::new(),
            |rows: &usize| {
                if *rows > 0 {
                    SpanStatus::ok()
                } else {
                    SpanStatus::error("no rows")
                }
            },
            async {
                yield_once().await;
                0_usize
            },
        );
        let outer_span_id = outer.span().context().span_id;
        outer.end(SpanStatus::unset());

        let mut task = pin!(task);
        assert_eq!(poll_once(task.as_mut()), Poll::Pending);
        assert_eq!(
            exported_names(exporter),
            ["outer"],
            "not ended while it waits"
        );
        assert_eq!(poll_once(task.as_mut()), Poll::Ready(0));

        let query = exported(exporter, "query");
        assert_eq!(query.parent_span_id, Some(outer_span_id));
        assert_eq!(query.kind, SpanKind::Client);
        assert_eq!(query.status, StatusCode::Error);
        assert_eq!(query.status_message, "no rows");
    });
}

// As `running_span`: the span ends as an error with no message, so no panic
// text reaches an exporter, and the panic continues to whoever polled.
#[test]
fn running_span_async_ends_a_panicking_future_as_an_error_and_resumes_the_panic() {
    with_tasks(|exporter| {
        let mut task = pin!(running_span_async(
            Parent::none(),
            "job",
            SpanKind::Internal,
            Attributes::new(),
            ends_ok::<()>,
            async {
                yield_once().await;
                panic!("a secret is in this message");
            },
        ));
        assert_eq!(poll_once(task.as_mut()), Poll::Pending);

        let unwound = panic::catch_unwind(AssertUnwindSafe(|| poll_once(task.as_mut())));

        let payload = unwound.expect_err("the future's panic reaches whoever polled");
        assert_eq!(
            payload.downcast_ref::<&str>(),
            Some(&"a secret is in this message")
        );
        assert_eq!(current_span_context(), None);
        let job = exported(exporter, "job");
        assert_eq!(job.status, StatusCode::Error);
        assert_eq!(job.status_message, "");
    });
}

// A future dropped before it completes — a cancelled task — ends its span with
// the status it has; nothing is left open or current.
#[test]
fn a_cancelled_running_span_async_ends_its_span_with_the_status_it_has() {
    with_tasks(|exporter| {
        {
            let mut task = Box::pin(running_span_async(
                Parent::none(),
                "cancelled",
                SpanKind::Internal,
                Attributes::new(),
                ends_ok,
                yield_once(),
            ));
            assert_eq!(poll_once(task.as_mut()), Poll::Pending);
        }
        assert_eq!(current_span_context(), None);
        assert_eq!(exported(exporter, "cancelled").status, StatusCode::Unset);
    });
}

// A task's wrapper is printed by what it carries that can be printed: its
// span's ids. The future inside need not be `Debug`.
#[test]
fn a_tasks_wrapper_debugs_as_its_spans_ids() {
    with_tasks(|_| {
        let span = open_root("task");
        let wrapped = with_span_async(&span, async {});
        let printed = format!("{wrapped:?}");
        assert!(printed.starts_with("InSpan"), "{printed}");
        assert!(
            printed.contains(&format!("{:?}", span.context())),
            "{printed}"
        );
    });
}
