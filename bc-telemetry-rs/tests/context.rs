//! Ported from `libs/platform/telemetry/test/context_test.cpp`.

mod support;

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use bc_date_time::{Epoch, EpochSource};
use bc_telemetry::{
    AttributeValue, Attributes, InMemoryExporter, LogRecord, MetricPoint, Parent, PipelineBuilder,
    Severity, Span, SpanGuard, SpanKind, SpanStatus, StatusCode, TelemetryExporter, add_span_event,
    add_span_event_if_any, current_span_context, drop_counts, dropped_count, ends_ok, record_log,
    running_span, set_span_attribute, set_span_attribute_if_any, set_span_status,
    set_span_status_if_any, start_span,
};

use support::{
    AssertionCapture, counting_id_source, exported_names, pipeline_lock, ticking_clock_from,
    with_in_memory, with_no_pipeline,
};

/// An internal span under the current one, with no attributes: the shape most
/// of these cases start.
fn start_child(name: &str) -> SpanGuard {
    start_span(
        Parent::current(),
        name,
        SpanKind::Internal,
        Attributes::new(),
    )
}

#[test]
fn no_active_span() {
    with_no_pipeline(|| {
        assert!(current_span_context().is_none());
    });
}

#[test]
fn start_span_creates_context() {
    with_in_memory(|_| {
        let guard = start_child("work");
        let context = current_span_context().expect("a span is active");
        assert_eq!(context, guard.span().context());
        assert!(context.trace_id.is_valid());
        assert!(context.span_id.is_valid());
        assert!(context.is_sampled());
    });
}

#[test]
fn span_guard_cleans_up() {
    with_in_memory(|_| {
        {
            let _guard = start_child("scoped");
            assert!(current_span_context().is_some());
        }
        assert!(current_span_context().is_none());
    });
}

#[test]
fn span_exported_on_guard_destruction() {
    with_in_memory(|exporter| {
        {
            let _guard = start_span(
                Parent::current(),
                "exported",
                SpanKind::Client,
                Attributes::new(),
            );
            assert!(exporter.spans().is_empty(), "not exported until it ends");
        }
        let spans = exporter.spans();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].name, "exported");
        assert_eq!(spans[0].kind, SpanKind::Client);
    });
}

#[test]
fn nested_spans_share_a_trace_id() {
    with_in_memory(|_| {
        let outer = start_child("outer");
        let inner = start_child("inner");
        assert_eq!(
            outer.span().context().trace_id,
            inner.span().context().trace_id
        );
        assert_ne!(
            outer.span().context().span_id,
            inner.span().context().span_id
        );
    });
}

#[test]
fn a_nested_span_has_a_parent() {
    with_in_memory(|exporter| {
        let outer_span_id = {
            let outer = start_child("outer");
            {
                let _inner = start_child("inner");
            }
            outer.span().context().span_id
        };
        let spans = exporter.spans();
        let inner = spans
            .iter()
            .find(|span| span.name == "inner")
            .expect("inner ended");
        assert_eq!(inner.parent_span_id, Some(outer_span_id));
    });
}

#[test]
fn a_root_span_has_no_parent() {
    with_in_memory(|exporter| {
        {
            let _root = start_child("root");
        }
        assert_eq!(exporter.spans()[0].parent_span_id, None);
    });
}

// `running_span` is what `with_span(name, body)` was: start a span, run the
// body, end the span. The name moved because `with_span` now means running
// under a span some other code controls (`tests/parenting.rs`).

#[test]
fn running_span_returns_the_bodys_value() {
    with_in_memory(|exporter| {
        let result = running_span(
            Parent::current(),
            "compute",
            SpanKind::Internal,
            Attributes::new(),
            ends_ok,
            |_span| 6 * 7,
        );
        assert_eq!(result, 42);
        assert_eq!(exported_names(exporter), ["compute"]);
    });
}

#[test]
fn running_span_accepts_a_body_returning_nothing() {
    with_in_memory(|exporter| {
        let mut ran = false;
        running_span(
            Parent::current(),
            "side-effect",
            SpanKind::Internal,
            Attributes::new(),
            ends_ok,
            |_span| ran = true,
        );
        assert!(ran);
        assert_eq!(exporter.spans().len(), 1);
    });
}

#[test]
fn running_span_carries_kind_and_attributes() {
    with_in_memory(|exporter| {
        running_span(
            Parent::current(),
            "db-query",
            SpanKind::Client,
            vec![("db.system".to_owned(), AttributeValue::from("postgres"))],
            ends_ok,
            |_span| {},
        );
        let spans = exporter.spans();
        assert_eq!(spans[0].kind, SpanKind::Client);
        assert_eq!(
            spans[0].attributes,
            [("db.system".to_owned(), AttributeValue::from("postgres"))]
        );
    });
}

#[test]
fn add_span_event_records_an_event() {
    with_in_memory(|exporter| {
        {
            let _outer = start_child("outer");
            add_span_event(
                "cache.miss",
                vec![("key".to_owned(), AttributeValue::from("k"))],
            );
        }
        let spans = exporter.spans();
        assert_eq!(spans[0].events.len(), 1);
        assert_eq!(spans[0].events[0].name, "cache.miss");
        assert_eq!(spans[0].events[0].attributes.len(), 1);
    });
}

#[test]
fn set_span_status_records_status_and_message() {
    with_in_memory(|exporter| {
        {
            let _outer = start_child("outer");
            set_span_status(SpanStatus::error("timed out"));
        }
        let spans = exporter.spans();
        assert_eq!(spans[0].status, StatusCode::Error);
        assert_eq!(spans[0].status_message, "timed out");
    });
}

#[test]
fn set_span_attribute_appends_an_attribute() {
    with_in_memory(|exporter| {
        {
            let _outer = start_child("outer");
            set_span_attribute("retries", 2_i64);
            set_span_attribute("cached", true);
        }
        let attributes = &exporter.spans()[0].attributes;
        assert_eq!(
            *attributes,
            [
                ("retries".to_owned(), AttributeValue::Int64(2)),
                ("cached".to_owned(), AttributeValue::Boolean(true)),
            ]
        );
    });
}

// A mutation of the current span with no span current is a misuse, not a no-op:
// the caller meant to say something about a span that is not there. It never
// fails the caller — it is counted and reported to the assertion handler,
// naming the call.
#[test]
fn mutating_the_current_span_with_none_current_is_counted_and_reported() {
    with_in_memory(|exporter| {
        let assertions = AssertionCapture::install();
        let here = std::panic::Location::caller();

        add_span_event("orphan", Attributes::new());
        set_span_status(SpanStatus::error(""));
        set_span_attribute("key", "value");

        assert!(current_span_context().is_none());
        assert!(exporter.spans().is_empty(), "{:?}", exporter.spans());
        assert_eq!(drop_counts().misuse, 3);
        assert_eq!(dropped_count(), 3);
        let failures = assertions.failures();
        assert_eq!(failures.len(), 3);
        for failure in &failures {
            assert_eq!(failure.expectation, "a span is active");
            assert_eq!(failure.location.file(), here.file());
        }
        assert_eq!(failures[0].detail, "add_span_event(\"orphan\")");
        assert_eq!(failures[1].detail, "set_span_status");
        // The key names the call; the value never leaves the caller.
        assert_eq!(failures[2].detail, "set_span_attribute(\"key\")");
        // Each names its own line: the three calls above, in order.
        assert_eq!(failures[0].location.line(), here.line() + 2);
        assert_eq!(failures[1].location.line(), here.line() + 3);
        assert_eq!(failures[2].location.line(), here.line() + 4);
    });
}

// The `*_if_any` forms are for a caller whose span is optional: with none
// current they do nothing, count nothing and report nothing — and with one
// current they are the plain forms.
#[test]
fn the_if_any_forms_do_nothing_without_a_span_and_mutate_the_current_one_with_it() {
    with_in_memory(|exporter| {
        let assertions = AssertionCapture::install();

        add_span_event_if_any("orphan", Attributes::new());
        set_span_status_if_any(SpanStatus::error(""));
        set_span_attribute_if_any("key", "value");
        assert_eq!(dropped_count(), 0);
        assert!(
            assertions.failures().is_empty(),
            "{:?}",
            assertions.failures()
        );

        {
            let _guard = start_child("request");
            add_span_event_if_any("noted", Attributes::new());
            set_span_status_if_any(SpanStatus::error("refused"));
            set_span_attribute_if_any("key", "value");
        }
        let spans = exporter.spans();
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].events.len(), 1);
        assert_eq!(spans[0].events[0].name, "noted");
        assert_eq!(spans[0].status, StatusCode::Error);
        assert_eq!(spans[0].status_message, "refused");
        assert_eq!(spans[0].attributes.len(), 1);
        assert!(
            assertions.failures().is_empty(),
            "{:?}",
            assertions.failures()
        );
    });
}

#[test]
fn moving_a_guard_ends_its_span_exactly_once() {
    // Rewritten from `SpanGuardMoveWorks`. The C++ needs its moved-from guard
    // to give up its activation so the destructor ends nothing; Rust's moves
    // are destructive, so the moved-from guard does not exist to be dropped.
    // What survives is the observable: exactly one span, ended when the new
    // owner drops, and current until then.
    with_in_memory(|exporter| {
        let started = start_child("moved");
        let moved = started;
        assert!(exporter.spans().is_empty(), "the move did not end it");
        assert_eq!(current_span_context(), Some(moved.span().context()));
        drop(moved);
        assert_eq!(exporter.spans().len(), 1);
        assert!(current_span_context().is_none());
    });
}

#[test]
fn assigning_over_an_active_guard_ends_the_displaced_span() {
    // Rewritten from `MoveAssignOverActiveSpanEndsCurrentSpan`. Assigning to a
    // `Drop` value drops the old one, so the C++'s hand-written move assignment
    // has no analogue; the observable is what is pinned.
    with_in_memory(|exporter| {
        let mut guard = start_child("first");
        let first_span_id = guard.span().context().span_id;
        assert!(exporter.spans().is_empty(), "{:?}", exporter.spans());

        // The right side starts while "first" is still current, so "second"
        // is its child; then "first" is dropped and exported.
        guard = start_child("second");
        assert_eq!(exported_names(exporter), ["first"]);
        assert_eq!(current_span_context(), Some(guard.span().context()));

        drop(guard);
        let spans = exporter.spans();
        assert_eq!(exported_names(exporter), ["first", "second"]);
        // "second" keeps "first" as its parent although "first" ended first.
        assert_eq!(spans[1].parent_span_id, Some(first_span_id));
        assert!(current_span_context().is_none());
    });
}

#[test]
fn ending_an_outer_guard_while_an_inner_is_held_does_not_disturb_the_inner() {
    // An activation is taken back by token, so ending a span that is not the
    // newest shifts nothing under the guards above it.
    with_in_memory(|exporter| {
        let inner = {
            let _outer = start_child("outer");
            start_child("inner")
            // `outer` ends here, from under `inner`.
        };
        let inner_span_id = inner.span().context().span_id;
        assert_eq!(exported_names(exporter), ["outer"]);

        // The inner span is still live and still itself.
        assert_eq!(
            current_span_context().map(|context| context.span_id),
            Some(inner_span_id)
        );

        drop(inner);
        let spans = exporter.spans();
        assert_eq!(exported_names(exporter), ["outer", "inner"]);
        assert_eq!(spans[1].context.span_id, inner_span_id);
        assert!(current_span_context().is_none());
    });
}

#[test]
fn three_deep_nesting() {
    with_in_memory(|exporter| {
        {
            let outer = start_child("a");
            let middle = start_child("b");
            let inner = start_child("c");

            assert_eq!(
                outer.span().context().trace_id,
                inner.span().context().trace_id
            );
            assert_eq!(
                middle.span().context().trace_id,
                inner.span().context().trace_id
            );
            assert_eq!(current_span_context(), Some(inner.span().context()));
        }
        // Guards drop in reverse declaration order, so the innermost ends first.
        let spans = exporter.spans();
        assert_eq!(exported_names(exporter), ["c", "b", "a"]);
        assert_eq!(spans[0].parent_span_id, Some(spans[1].context.span_id));
        assert_eq!(spans[1].parent_span_id, Some(spans[2].context.span_id));
    });
}

// --- Exporter re-entrancy ---
//
// A guard gives its activation back, and its span is marked ended with its data
// moved out, BEFORE the clock and the exporter are called — so an exporter that
// starts and ends spans of its own during export cannot corrupt the context it
// is called from, and finds no lock held and no borrow taken. Each case also
// expects no misuse report.

/// Wraps an in-memory exporter and runs a hook on the way into
/// `export_spans`. The latch is what makes the fixture usable at all: a span the
/// hook starts is itself exported, so without it the hook would re-enter itself
/// without bound.
struct ReentrantExporter {
    inner: InMemoryExporter,
    on_export_spans: Box<dyn Fn() + Send + Sync>,
    reentered: AtomicBool,
}

impl std::fmt::Debug for ReentrantExporter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ReentrantExporter")
    }
}

impl TelemetryExporter for ReentrantExporter {
    fn export_spans(&self, spans: &[Span]) {
        if !self.reentered.swap(true, Ordering::SeqCst) {
            (self.on_export_spans)();
            self.reentered.store(false, Ordering::SeqCst);
        }
        self.inner.export_spans(spans);
    }
    fn export_metrics(&self, metrics: &[MetricPoint]) {
        self.inner.export_metrics(metrics);
    }
    fn export_logs(&self, logs: &[LogRecord]) {
        self.inner.export_logs(logs);
    }
    fn flush(&self) {
        self.inner.flush();
    }
    fn dropped(&self) -> u64 {
        self.inner.dropped()
    }
}

/// Installs a [`ReentrantExporter`] running `hook`, runs `body`, and tears the
/// pipeline down — under the one-pipeline lock.
fn with_reentrant_exporter(
    hook: impl Fn() + Send + Sync + 'static,
    body: impl FnOnce(&ReentrantExporter),
) {
    let _serialized = pipeline_lock();
    let exporter = Arc::new(ReentrantExporter {
        inner: InMemoryExporter::new(),
        on_export_spans: Box::new(hook),
        reentered: AtomicBool::new(false),
    });
    let _pipeline = PipelineBuilder::new(exporter.clone())
        .epoch_source(ticking_clock_from(1_000))
        .id_source(counting_id_source())
        .install();
    body(&exporter);
}

#[test]
fn an_exporter_that_starts_and_ends_a_nested_span_keeps_the_context_balanced() {
    with_reentrant_exporter(
        || {
            let _nested = start_child("nested");
            let _child = start_child("child");
        },
        |exporter| {
            let assertions = AssertionCapture::install();
            {
                let _root = start_child("root");
            }

            // The context is balanced and nothing is left behind.
            assert!(current_span_context().is_none());

            let spans = exporter.inner.spans();
            // The hook's own spans export first: they finish inside the export
            // of "root", and the latch keeps their exports out of the hook.
            assert_eq!(exported_names(&exporter.inner), ["child", "nested", "root"]);
            // "root" is no longer current when the hook runs — it left the
            // context before its export — so "nested" is the root of a NEW
            // trace rather than root's child. The ordering's consequence,
            // pinned rather than discovered later.
            assert_eq!(spans[0].parent_span_id, Some(spans[1].context.span_id));
            assert_eq!(spans[1].parent_span_id, None);
            assert_eq!(spans[2].parent_span_id, None);
            assert_ne!(spans[1].context.trace_id, spans[2].context.trace_id);
            assert!(
                assertions.failures().is_empty(),
                "{:?}",
                assertions.failures()
            );
        },
    );
}

thread_local! {
    /// Where the hook below parks a guard. A guard is not `Send`, so an
    /// exporter — which is — cannot hold one, as the C++ fixture's does; this
    /// thread's slot can, since the hook runs on the thread that ends "root".
    static HELD: RefCell<Option<SpanGuard>> = const { RefCell::new(None) };
}

#[test]
fn an_exporter_that_holds_a_guard_across_the_export_ends_it_later() {
    let fired = Arc::new(AtomicBool::new(false));
    let fired_in_hook = Arc::clone(&fired);
    with_reentrant_exporter(
        move || {
            // One-shot: the held span's own export re-enters the hook once the
            // latch clears, and a hook that started another span there would
            // never stop.
            if !fired_in_hook.swap(true, Ordering::SeqCst) {
                HELD.with(|held| *held.borrow_mut() = Some(start_child("held")));
            }
        },
        |exporter| {
            let assertions = AssertionCapture::install();
            {
                let _root = start_child("root");
            }

            // "root" exported; "held" is still open, parked by the hook.
            assert_eq!(exported_names(&exporter.inner), ["root"]);
            assert!(current_span_context().is_some());

            // Released here, while the pipeline lives, so its export can be
            // asserted.
            drop(HELD.with(|held| held.borrow_mut().take()));

            assert_eq!(exported_names(&exporter.inner), ["root", "held"]);
            assert!(current_span_context().is_none());
            assert!(
                assertions.failures().is_empty(),
                "{:?}",
                assertions.failures()
            );
        },
    );
    assert!(fired.load(Ordering::SeqCst));
}

#[test]
fn a_clock_that_starts_a_span_does_not_invalidate_the_ending_span() {
    // The hazard one callback earlier: ending a span reads the clock to stamp
    // its end, and starting a span reads it too, so this needs its own latch.
    // The assertions pin the exact shape — a corrupted end time, a lost span or
    // a wrong order each fail here.
    let _serialized = pipeline_lock();
    let exporter = Arc::new(InMemoryExporter::new());
    let in_clock = Arc::new(AtomicBool::new(false));
    let next = Arc::new(AtomicI64::new(1_000));
    let clock = EpochSource::new(move || {
        if !in_clock.swap(true, Ordering::SeqCst) {
            {
                let _sneaky = start_child("from-the-clock");
            }
            in_clock.store(false, Ordering::SeqCst);
        }
        Epoch::from_millis(next.fetch_add(1, Ordering::SeqCst))
    });
    let _pipeline = PipelineBuilder::new(exporter.clone())
        .epoch_source(clock)
        .id_source(counting_id_source())
        .install();
    let assertions = AssertionCapture::install();

    {
        let _root = start_child("root");
    }

    assert!(current_span_context().is_none());
    // THREE, not two: the clock is read once when "root" starts and again when
    // it ends, and the latch suppresses re-entry only within one read — so the
    // hook runs twice and each run contributes a span of its own.
    assert_eq!(
        exported_names(&exporter),
        ["from-the-clock", "from-the-clock", "root"]
    );
    let spans = exporter.spans();
    assert!(spans[2].context.span_id.is_valid());
    // The clock is deterministic and monotonic, so a stamp read wrongly shows
    // as an out-of-order or repeated value rather than passing.
    for span in &spans {
        assert!(span.start_time < span.end_time, "{}", span.name);
    }
    assert!(
        assertions.failures().is_empty(),
        "{:?}",
        assertions.failures()
    );
}

#[test]
fn an_exporter_that_records_a_log_during_export_does_not_recurse_or_deadlock() {
    // Characterization, not a negative proof: `record_log` reaches no span
    // state, so this held before the parenting port too. It is here because
    // "an exporter may record" is the contract the module states, and an
    // unexercised contract is a claim.
    with_reentrant_exporter(
        || {
            record_log(
                Parent::current(),
                Severity::Info,
                "from-exporter",
                Vec::new(),
            );
        },
        |exporter| {
            let assertions = AssertionCapture::install();
            {
                let _root = start_child("root");
            }
            assert_eq!(exported_names(&exporter.inner), ["root"]);
            assert_eq!(exporter.inner.logs().len(), 1);
            assert!(
                assertions.failures().is_empty(),
                "{:?}",
                assertions.failures()
            );
        },
    );
}
