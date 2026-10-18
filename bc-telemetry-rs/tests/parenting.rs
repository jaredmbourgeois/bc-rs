//! Ported from `libs/platform/telemetry/test/parenting_test.cpp`: which span a
//! new span, a log record or a metric point belongs to; which span is current;
//! a span held rather than scoped; sampling that follows the parent; running
//! code in a span; and telemetry that never fails its caller.
//!
//! Where the C++ passes a default — the current parent, an internal kind, the
//! result type's end status — these pass the value, since nothing here has a
//! default to take (`src/context.rs`, "What else differs").

mod support;

use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use bc_core::{AssertionHandler, set_assertion_handler};
use bc_date_time::{Epoch, EpochSource};
use bc_telemetry::{
    AttributeValue, Attributes, DropCounts, IdSource, InMemoryExporter, LogRecord, MetricPoint,
    OpenSpan, Parent, PipelineBuilder, Sampler, SamplingDecision, Severity, Span, SpanContext,
    SpanGuard, SpanId, SpanKind, SpanStatus, StatusCode, TRACE_FLAG_SAMPLED, TelemetryExporter,
    TraceId, add_span_event, always_on_sampler, current_span_context, drop_counts, dropped_count,
    ends_ok, open_span, record_counter, record_gauge, record_histogram, record_log, running_span,
    set_span_attribute_if_any, shutdown_pipeline, start_span, with_span, with_span_ending,
};

use support::{
    AssertionCapture, clock_at, counting_id_source, exported, exported_names, pipeline_lock,
    ticking_clock_from, with_in_memory_configured, with_no_pipeline,
};

/// The pipeline every `ParentingTest` case runs under: an in-memory exporter,
/// every span sampled, and a clock that advances one millisecond per read.
fn with_parenting(body: impl FnOnce(&Arc<InMemoryExporter>)) {
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

/// An internal span opened under `parent`, with no attributes.
fn open_under(parent: Parent, name: &str) -> OpenSpan {
    open_span(parent, name, SpanKind::Internal, Attributes::new())
}

/// A context as a `traceparent` from another process would carry it.
fn remote_context(trace_byte: u8, span_byte: u8, trace_flags: u8) -> SpanContext {
    let mut trace_id = TraceId::default();
    trace_id.bytes[0] = trace_byte;
    let mut span_id = SpanId::default();
    span_id.bytes[0] = span_byte;
    SpanContext {
        trace_id,
        span_id,
        trace_flags,
    }
}

fn counts(unsampled: u64, failed: u64, misuse: u64) -> DropCounts {
    DropCounts {
        unsampled,
        failed,
        misuse,
        identity: 0,
    }
}

// fidelity-compile-proof: `ParentingTest.MutatingASpanAfterItEndedIsCountedAndReported`,
// `ParentingTest.WithSpanOnAnEndedSpanRunsTheBodyAndReportsTheMisuse` and
// `ParentingTest.ASpanThatEndsWhileItIsLentIsReported`. Ending a span takes its
// handle by value, so after `end` no handle is left to mutate or lend, and while
// `with_span` borrows one there is none to end. These two items pin that
// signature: were `end` to take `&mut self`, neither would compile.
const _: fn(OpenSpan, SpanStatus) = OpenSpan::end;
const _: fn(SpanGuard, SpanStatus) = SpanGuard::end;

// --- Which span a new span belongs to ---

#[test]
fn a_given_parent_is_used_whatever_span_is_current() {
    with_parenting(|exporter| {
        let outer = start_child("outer");
        let inner = start_child("inner");
        {
            // `inner` is current; the new span names `outer` and belongs to it.
            let sibling = start_span(
                Parent::span(outer.span()),
                "sibling",
                SpanKind::Internal,
                Attributes::new(),
            );
            assert_eq!(current_span_context(), Some(sibling.span().context()));
        }
        drop(inner);
        drop(outer);

        let sibling = exported(exporter, "sibling");
        let outer = exported(exporter, "outer");
        assert_eq!(sibling.parent_span_id, Some(outer.context.span_id));
        assert_ne!(
            sibling.parent_span_id,
            Some(exported(exporter, "inner").context.span_id)
        );
        assert_eq!(sibling.context.trace_id, outer.context.trace_id);
    });
}

#[test]
fn no_parent_starts_a_new_trace_even_inside_a_span() {
    with_parenting(|exporter| {
        let outer = start_child("outer");
        {
            let _detached = start_span(
                Parent::none(),
                "detached",
                SpanKind::Internal,
                Attributes::new(),
            );
            // It is current while it lives, so what starts inside it belongs to it.
            let _child = start_child("child-of-detached");
        }
        drop(outer);

        let detached = exported(exporter, "detached");
        assert_eq!(detached.parent_span_id, None);
        assert_ne!(
            detached.context.trace_id,
            exported(exporter, "outer").context.trace_id
        );
        assert_eq!(
            exported(exporter, "child-of-detached").parent_span_id,
            Some(detached.context.span_id)
        );
    });
}

#[test]
fn the_current_parent_with_nothing_current_is_a_root_and_never_a_misuse() {
    with_parenting(|exporter| {
        let assertions = AssertionCapture::install();
        drop(start_child("root"));

        assert_eq!(exported(exporter, "root").parent_span_id, None);
        assert!(
            assertions.failures().is_empty(),
            "{:?}",
            assertions.failures()
        );
        assert_eq!(dropped_count(), 0);
    });
}

#[test]
fn a_remote_parent_continues_its_trace() {
    with_parenting(|exporter| {
        let remote = remote_context(0xAA, 0xBB, TRACE_FLAG_SAMPLED);
        drop(start_span(
            Parent::remote(remote),
            "incoming",
            SpanKind::Server,
            Attributes::new(),
        ));

        let incoming = exported(exporter, "incoming");
        assert_eq!(incoming.parent_span_id, Some(remote.span_id));
        assert_eq!(incoming.context.trace_id, remote.trace_id);
        assert_ne!(incoming.context.span_id, remote.span_id);
        assert_eq!(incoming.kind, SpanKind::Server);
    });
}

// A context that parsed to nothing names no span. Parenting to it would export
// a span whose trace id is all zeros, which no collector accepts.
#[test]
fn a_remote_parent_with_no_valid_ids_is_no_parent() {
    with_parenting(|exporter| {
        drop(start_span(
            Parent::remote(SpanContext::default()),
            "orphan",
            SpanKind::Internal,
            Attributes::new(),
        ));

        let orphan = exported(exporter, "orphan");
        assert_eq!(orphan.parent_span_id, None);
        assert!(orphan.context.trace_id.is_valid());
        assert_eq!(Parent::remote(SpanContext::default()).context(), None);
        // Either id missing is enough: a trace with no span names no parent,
        // and neither does a span in no trace.
        let mut no_span = remote_context(0xAA, 0, TRACE_FLAG_SAMPLED);
        assert_eq!(Parent::remote(no_span).context(), None);
        no_span = remote_context(0, 0xBB, TRACE_FLAG_SAMPLED);
        assert_eq!(Parent::remote(no_span).context(), None);
    });
}

// A span's ids are facts about it, so they outlive it: a child may start after
// its parent has ended, and nothing about that is a misuse. Ending takes the
// handle, so the parent is taken from it first — a value that outlives it.
#[test]
fn a_parent_taken_from_a_span_outlives_the_span() {
    with_parenting(|exporter| {
        let assertions = AssertionCapture::install();
        let parent_span = start_child("parent");
        let parent = Parent::span(parent_span.span());
        parent_span.end(SpanStatus::unset());

        drop(start_span(
            parent,
            "late-child",
            SpanKind::Internal,
            Attributes::new(),
        ));

        let late = exported(exporter, "late-child");
        let parent = exported(exporter, "parent");
        assert_eq!(late.parent_span_id, Some(parent.context.span_id));
        assert_eq!(late.context.trace_id, parent.context.trace_id);
        assert!(
            assertions.failures().is_empty(),
            "{:?}",
            assertions.failures()
        );
        assert_eq!(dropped_count(), 0);
    });
}

// A parent is a value, read when it is asked for. What becomes current
// afterwards does not change it — which is what lets it be carried to work that
// runs later or elsewhere.
#[test]
fn a_parent_is_a_value_taken_when_it_is_asked() {
    with_parenting(|exporter| {
        let outer = start_child("outer");
        let taken = Parent::current();

        let inner = start_child("inner");
        drop(start_span(
            taken,
            "by-start-span",
            SpanKind::Internal,
            Attributes::new(),
        ));
        running_span(
            taken,
            "by-running-span",
            SpanKind::Internal,
            Attributes::new(),
            ends_ok,
            |_span| {},
        );
        drop(inner);
        drop(outer);

        let outer = exported(exporter, "outer");
        assert_eq!(
            exported(exporter, "by-start-span").parent_span_id,
            Some(outer.context.span_id)
        );
        assert_eq!(
            exported(exporter, "by-running-span").parent_span_id,
            Some(outer.context.span_id)
        );
        assert_eq!(taken.context(), Some(outer.context));
    });
}

// --- Which span is current ---

#[test]
fn ending_an_outer_span_first_leaves_the_inner_one_current() {
    with_parenting(|exporter| {
        let outer = start_child("outer");
        let inner = start_child("inner");

        outer.end(SpanStatus::unset());
        assert_eq!(current_span_context(), Some(inner.span().context()));
        drop(start_child("child"));
        inner.end(SpanStatus::unset());
        assert_eq!(current_span_context(), None);

        assert_eq!(
            exported(exporter, "child").parent_span_id,
            Some(exported(exporter, "inner").context.span_id)
        );
    });
}

// `with_span` takes back its own activation and no other: a span the body
// started and handed out is still open, and a guard's span stays current until
// the guard ends.
#[test]
fn with_span_takes_back_only_its_own_activation() {
    with_parenting(|_| {
        let outer = start_child("outer");

        let escaped = with_span(outer.span(), |_lent| start_child("escaped"));
        assert_eq!(current_span_context(), Some(escaped.span().context()));

        escaped.end(SpanStatus::unset());
        assert_eq!(current_span_context(), Some(outer.span().context()));
    });
}

#[test]
fn a_span_activated_twice_stays_current_until_the_outer_activation_is_taken_back() {
    with_parenting(|_| {
        let outer = start_child("outer");
        let inner = start_child("inner");

        with_span(outer.span(), |lent| {
            with_span(lent, |_again| {});
            // The inner `with_span` took back its own activation, not this one.
            assert_eq!(current_span_context(), Some(lent.context()));
        });
        assert_eq!(current_span_context(), Some(inner.span().context()));
    });
}

// --- A span that is held, not scoped ---

#[test]
fn an_open_span_is_current_nowhere_until_it_is_lent() {
    with_parenting(|exporter| {
        let held = open_under(Parent::current(), "held");
        assert_eq!(current_span_context(), None);
        // Not its child: `held` is open, and nothing has made it current.
        drop(start_child("unrelated"));

        with_span(&held, |lent| {
            assert_eq!(current_span_context(), Some(lent.context()));
            drop(start_child("child"));
        });
        assert_eq!(current_span_context(), None);
        drop(held);

        assert_eq!(exported(exporter, "unrelated").parent_span_id, None);
        assert_eq!(
            exported(exporter, "child").parent_span_id,
            Some(exported(exporter, "held").context.span_id)
        );
    });
}

// Not being current is about what an open span is to OTHER spans. Its own
// parent is chosen like any span's.
#[test]
fn an_open_span_starts_under_the_parent_it_is_given() {
    with_parenting(|exporter| {
        let outer = start_child("outer");
        drop(open_under(Parent::current(), "held"));
        drop(open_under(Parent::none(), "apart"));
        drop(outer);

        let outer = exported(exporter, "outer");
        let held = exported(exporter, "held");
        assert_eq!(held.parent_span_id, Some(outer.context.span_id));
        assert_eq!(held.context.trace_id, outer.context.trace_id);
        assert_eq!(exported(exporter, "apart").parent_span_id, None);
    });
}

#[test]
fn releasing_a_guard_keeps_its_span_open_and_out_of_the_context() {
    with_parenting(|exporter| {
        let assertions = AssertionCapture::install();
        let guard = start_child("job");

        let held = guard.release();
        assert_eq!(current_span_context(), None);
        assert!(exporter.spans().is_empty(), "released, not ended");

        held.add_event("after-release", Attributes::new());
        held.end(SpanStatus::ok());

        let job = exported(exporter, "job");
        assert_eq!(job.events.len(), 1);
        assert_eq!(job.status, StatusCode::Ok);
        assert!(
            assertions.failures().is_empty(),
            "{:?}",
            assertions.failures()
        );
    });
}

// Ending takes the handle, so the ids are read while it is open; they are a
// value, and the span exports under exactly them.
#[test]
fn a_spans_ids_read_while_it_is_open_are_the_ids_it_exports() {
    with_parenting(|exporter| {
        let span = open_under(Parent::current(), "job");
        let while_open = span.context();
        span.end(SpanStatus::unset());

        assert_eq!(exported(exporter, "job").context, while_open);
    });
}

// Assigning over a span must not drop it unexported.
#[test]
fn assigning_over_an_open_span_ends_it_first() {
    with_parenting(|exporter| {
        let mut first = open_under(Parent::current(), "first");
        let first_context = first.context();
        let second = open_under(Parent::current(), "second");
        let second_context = second.context();

        first = second;

        assert_eq!(exported_names(exporter), ["first"]);
        assert_eq!(exported(exporter, "first").context, first_context);
        assert_eq!(first.context(), second_context);

        first.end(SpanStatus::unset());
        assert_eq!(exported(exporter, "second").context, second_context);
    });
}

// --- Sampling follows the parent ---

/// Installs a pipeline whose sampler answers by span name, keeping every span
/// it does not name, and runs `body` against its exporter.
fn with_sampling_by_name(
    decisions: &'static [(&'static str, SamplingDecision)],
    body: impl FnOnce(&Arc<InMemoryExporter>),
) {
    let sampler = Sampler::new(|_trace, name| {
        decisions
            .iter()
            .find(|(span_name, _)| *span_name == name)
            .map_or(SamplingDecision::RecordAndSample, |(_, decision)| *decision)
    });
    with_in_memory_configured(
        |builder| builder.sampler(sampler).epoch_source(clock_at(1)),
        body,
    );
}

#[test]
fn a_child_of_a_dropped_span_is_dropped_although_the_sampler_would_keep_it() {
    with_sampling_by_name(&[("root", SamplingDecision::Drop)], |exporter| {
        {
            let root = start_child("root");
            let child = start_child("child");
            assert!(!child.span().context().is_sampled());
            // The same whether the parent is the current span or a named one.
            let named_child = start_span(
                Parent::span(root.span()),
                "named-child",
                SpanKind::Internal,
                Attributes::new(),
            );
            assert!(!named_child.span().context().is_sampled());
        }
        // Never an orphan: a child whose parent is absent is absent too, and
        // all three are counted.
        assert!(exporter.spans().is_empty(), "{:?}", exporter.spans());
        assert_eq!(drop_counts(), counts(3, 0, 0));
    });
}

#[test]
fn the_sampler_may_narrow_a_kept_trace() {
    with_sampling_by_name(&[("noisy", SamplingDecision::Drop)], |exporter| {
        {
            let _root = start_child("root");
            let _noisy = start_child("noisy");
        }
        assert_eq!(exported_names(exporter), ["root"]);
        assert_eq!(dropped_count(), 1);
    });
}

#[test]
fn a_record_only_span_is_exported_unadvertised_and_so_are_its_children() {
    with_sampling_by_name(&[("root", SamplingDecision::RecordOnly)], |exporter| {
        {
            let root = start_child("root");
            assert!(!root.span().context().is_sampled());
            let child = start_child("child");
            // Recorded because its parent is; unsampled because its parent's
            // flag is clear.
            assert!(!child.span().context().is_sampled());
        }
        assert_eq!(exported_names(exporter), ["child", "root"]);
        assert_eq!(dropped_count(), 0);
    });
}

// A parent in this process says exactly whether it is exported, even when it is
// carried as a value to wherever its child starts. The wire carries only the
// sampled flag, which a `RecordOnly` span leaves clear — so the same span named
// through its bare context is one whose children are dropped.
#[test]
fn a_parent_in_this_process_carries_whether_it_is_exported_not_only_its_flag() {
    with_sampling_by_name(&[("root", SamplingDecision::RecordOnly)], |exporter| {
        {
            let root = start_child("root");
            let carried = Parent::current();
            drop(open_under(carried, "by-value"));
            drop(open_under(Parent::remote(root.span().context()), "by-wire"));
        }
        assert_eq!(exported_names(exporter), ["by-value", "root"]);
        assert_eq!(drop_counts(), counts(1, 0, 0));
    });
}

#[test]
fn an_unsampled_remote_parent_is_not_overruled_by_the_sampler() {
    with_sampling_by_name(&[], |exporter| {
        let unsampled_remote = remote_context(0x01, 0x02, 0);
        drop(start_span(
            Parent::remote(unsampled_remote),
            "incoming",
            SpanKind::Server,
            Attributes::new(),
        ));

        assert!(exporter.spans().is_empty(), "{:?}", exporter.spans());
        assert_eq!(dropped_count(), 1);
    });
}

#[test]
fn a_sampler_that_panics_drops_the_span_and_never_fails_the_caller() {
    with_in_memory_configured(
        |builder| builder.sampler(Sampler::new(|_, _| panic!("the sampler is broken"))),
        |exporter| {
            // Reaching the next line is the "never fails the caller" half.
            drop(start_child("unsampled"));
            assert!(exporter.spans().is_empty(), "{:?}", exporter.spans());
            assert_eq!(drop_counts(), counts(0, 1, 0));
        },
    );
}

/// An id source that panics leaves a span with no id: it is never exported
/// under the nil id in its place, and its end counts it as a failure — whether
/// the trace's id or the span's own was the one that could not be made.
#[test]
fn an_id_source_that_panics_fails_the_span_rather_than_export_a_nil_id() {
    with_in_memory_configured(
        |builder| builder.id_source(IdSource::new(|_| panic!("the id source is broken"))),
        |exporter| {
            // Installing reached this body, and reaching the next line is the
            // rest of the "never fails the caller" half.
            drop(start_child("unidentified"));
            assert!(exporter.spans().is_empty(), "{:?}", exporter.spans());
            assert_eq!(drop_counts(), counts(0, 1, 0));
            // With no producer id a log has no observation id, so it is
            // dropped and counted as one no writer could store once.
            record_log(
                Parent::none(),
                Severity::Info,
                "unidentified",
                Attributes::new(),
            );
            assert!(exporter.logs().is_empty(), "{:?}", exporter.logs());
            assert_eq!(drop_counts().identity, 1);
        },
    );
}

// A span that will never be exported is still a span — current, a parent, ended
// — but nothing is stored on it, so nothing is spent on it: no clock is read for
// its start, its events or its end.
#[test]
fn an_unexported_span_stores_nothing_and_reads_no_clock() {
    let clock_reads = Arc::new(AtomicUsize::new(0));
    let reads = Arc::clone(&clock_reads);
    with_in_memory_configured(
        |builder| {
            builder
                .sampler(Sampler::new(|_, _| SamplingDecision::Drop))
                .epoch_source(EpochSource::new(move || {
                    reads.fetch_add(1, Ordering::SeqCst);
                    Epoch::from_millis(1)
                }))
        },
        |_| {
            {
                let dropped = start_child("dropped");
                assert!(current_span_context().is_some());
                dropped.span().add_event("noted", Attributes::new());
                dropped.span().set_attribute("key", "value");
            }
            assert_eq!(clock_reads.load(Ordering::SeqCst), 0);
            assert_eq!(drop_counts(), counts(1, 0, 0));
        },
    );
}

// --- Mutating a given span ---

#[test]
fn a_guards_span_is_mutated_whatever_is_current() {
    with_parenting(|exporter| {
        let outer = start_child("outer");
        {
            let _inner = start_child("inner");
            outer.span().add_event("noted-on-outer", Attributes::new());
            outer.span().set_attribute("where", "outer");
            outer.span().set_status(SpanStatus::error("outer failed"));
        }
        drop(outer);

        let outer = exported(exporter, "outer");
        assert_eq!(outer.events.len(), 1);
        assert_eq!(outer.events[0].name, "noted-on-outer");
        assert_eq!(
            outer.attributes,
            [("where".to_owned(), AttributeValue::from("outer"))]
        );
        assert_eq!(outer.status, StatusCode::Error);
        assert_eq!(outer.status_message, "outer failed");

        let inner = exported(exporter, "inner");
        assert!(inner.events.is_empty(), "{:?}", inner.events);
        assert!(inner.attributes.is_empty(), "{:?}", inner.attributes);
        assert_eq!(inner.status, StatusCode::Unset);
    });
}

// Ending takes the guard, so a span ends exactly once; a second end is
// unwritable rather than a no-op.
#[test]
fn ending_a_guard_ends_its_span_with_the_status_given() {
    with_parenting(|exporter| {
        let assertions = AssertionCapture::install();
        let span = start_child("job");
        span.end(SpanStatus::error("timed out"));
        assert_eq!(current_span_context(), None);

        start_child("plain").end(SpanStatus::unset());

        let job = exported(exporter, "job");
        assert_eq!(job.status, StatusCode::Error);
        assert_eq!(job.status_message, "timed out");
        assert_eq!(exported(exporter, "plain").status, StatusCode::Unset);
        assert!(
            assertions.failures().is_empty(),
            "{:?}",
            assertions.failures()
        );
        assert_eq!(dropped_count(), 0);
    });
}

// An unset final status says nothing, so it changes nothing: the span keeps the
// status it was given while it ran.
#[test]
fn ending_with_an_unset_status_keeps_the_status_the_span_has() {
    with_parenting(|exporter| {
        let span = open_under(Parent::current(), "job");
        span.set_status(SpanStatus::error("refused"));
        span.end(SpanStatus::unset());

        let job = exported(exporter, "job");
        assert_eq!(job.status, StatusCode::Error);
        assert_eq!(job.status_message, "refused");
    });
}

// --- with_span: running under a span controlled elsewhere ---

#[test]
fn with_span_makes_the_span_current_for_the_body_and_ends_nothing() {
    with_parenting(|exporter| {
        let outer = start_child("outer");
        let inner = start_child("inner");

        let answer = with_span(outer.span(), |lent| {
            assert_eq!(current_span_context(), Some(lent.context()));
            let _child = start_child("child-of-outer");
            add_span_event("noted-on-the-current-span", Attributes::new());
            42
        });

        assert_eq!(answer, 42);
        // The activation is taken back: `inner` is current again, and `outer`
        // is still open.
        assert_eq!(current_span_context(), Some(inner.span().context()));
        assert_eq!(
            exported_names(exporter),
            ["child-of-outer"],
            "only the child has ended"
        );
        drop(inner);
        drop(outer);

        assert_eq!(
            exported(exporter, "child-of-outer").parent_span_id,
            Some(exported(exporter, "outer").context.span_id)
        );
    });
}

#[test]
fn with_span_hands_the_body_its_span() {
    with_parenting(|exporter| {
        let span = start_child("job");
        with_span(span.span(), |current| {
            current.add_event("from-the-body", Attributes::new());
        });
        drop(span);

        assert_eq!(exported(exporter, "job").events.len(), 1);
    });
}

#[test]
fn with_span_takes_its_activation_back_when_the_body_panics() {
    with_parenting(|exporter| {
        let outer = start_child("outer");
        let inner = start_child("inner");

        let unwound = panic::catch_unwind(AssertUnwindSafe(|| {
            with_span::<()>(outer.span(), |_lent| panic!("the body failed"));
        }));

        assert!(unwound.is_err());
        assert_eq!(current_span_context(), Some(inner.span().context()));
        // Borrowed, so not ended and not marked: whoever holds `outer` decides
        // what it means.
        assert!(exporter.spans().is_empty(), "{:?}", exporter.spans());
    });
}

#[test]
fn with_span_given_a_strategy_takes_the_span_over_and_ends_it_by_the_result() {
    with_parenting(|exporter| {
        let assertions = AssertionCapture::install();
        let span = start_child("job");
        let job_context = span.span().context();
        let later = start_child("started-later");

        // `started-later` is current, so the body sees `job` only because
        // `with_span_ending` made it so.
        let accepted = with_span_ending(
            span.release(),
            |result: &bool| {
                if *result {
                    SpanStatus::ok()
                } else {
                    SpanStatus::error("refused")
                }
            },
            |_job| {
                assert_eq!(current_span_context(), Some(job_context));
                false
            },
        );

        assert!(!accepted);
        assert_eq!(current_span_context(), Some(later.span().context()));
        let job = exported(exporter, "job");
        assert_eq!(job.status, StatusCode::Error);
        assert_eq!(job.status_message, "refused");
        assert!(
            assertions.failures().is_empty(),
            "{:?}",
            assertions.failures()
        );
    });
}

// --- running_span: a span that lives exactly as long as its body ---

#[test]
fn running_span_ends_a_body_that_says_nothing_as_ok() {
    with_parenting(|exporter| {
        running_span(
            Parent::current(),
            "job",
            SpanKind::Internal,
            Attributes::new(),
            ends_ok,
            |_span| {},
        );
        assert_eq!(exported(exporter, "job").status, StatusCode::Ok);
    });
}

#[test]
fn running_span_under_the_current_parent_is_its_child() {
    with_parenting(|exporter| {
        let outer = start_child("outer");
        running_span(
            Parent::current(),
            "inner",
            SpanKind::Internal,
            Attributes::new(),
            ends_ok,
            |_span| {},
        );
        drop(outer);

        let inner = exported(exporter, "inner");
        assert_eq!(
            inner.parent_span_id,
            Some(exported(exporter, "outer").context.span_id)
        );
        assert_eq!(inner.kind, SpanKind::Internal);
    });
}

/// A result type of this file's own, which knows its span status: what C++'s
/// `SpanOutcome` specialization is for, here a function the caller passes.
struct Verdict {
    accepted: bool,
}

impl Verdict {
    fn status(&self) -> SpanStatus {
        if self.accepted {
            SpanStatus::ok()
        } else {
            SpanStatus::error("rejected")
        }
    }
}

#[test]
fn running_span_maps_a_result_through_its_types_own_status() {
    with_parenting(|exporter| {
        let rejected = running_span(
            Parent::current(),
            "decide",
            SpanKind::Internal,
            Attributes::new(),
            Verdict::status,
            |_span| Verdict { accepted: false },
        );
        assert!(!rejected.accepted);
        let decide = exported(exporter, "decide");
        assert_eq!(decide.status, StatusCode::Error);
        assert_eq!(decide.status_message, "rejected");

        let accepted = running_span(
            Parent::current(),
            "decide-again",
            SpanKind::Internal,
            Attributes::new(),
            Verdict::status,
            |_span| Verdict { accepted: true },
        );
        assert!(accepted.accepted);
        assert_eq!(exported(exporter, "decide-again").status, StatusCode::Ok);
    });
}

#[test]
fn running_span_with_a_strategy_ends_by_what_the_strategy_says() {
    with_parenting(|exporter| {
        let rows = running_span(
            Parent::current(),
            "query",
            SpanKind::Client,
            vec![("db.system".to_owned(), AttributeValue::from("postgres"))],
            |count: &i32| {
                if *count > 0 {
                    SpanStatus::ok()
                } else {
                    SpanStatus::error("no rows")
                }
            },
            |_span| 0,
        );

        assert_eq!(rows, 0);
        let query = exported(exporter, "query");
        assert_eq!(query.kind, SpanKind::Client);
        assert_eq!(query.status, StatusCode::Error);
        assert_eq!(query.status_message, "no rows");
        assert_eq!(query.attributes.len(), 1);
    });
}

#[test]
fn running_span_hands_the_body_its_span_and_honours_the_parent_given() {
    with_parenting(|exporter| {
        let outer = start_child("outer");
        running_span(
            Parent::none(),
            "detached",
            SpanKind::Internal,
            Attributes::new(),
            ends_ok,
            |span| {
                span.add_event("inside", Attributes::new());
                assert_eq!(current_span_context(), Some(span.context()));
            },
        );
        drop(outer);

        let detached = exported(exporter, "detached");
        assert_eq!(detached.parent_span_id, None);
        assert_eq!(detached.events.len(), 1);
    });
}

// The body's panic is the caller's: the span ends as an error — with no
// message, so no panic text reaches an exporter — and the panic continues.
#[test]
fn running_span_ends_a_panicking_body_as_an_error_and_resumes_the_panic() {
    with_parenting(|exporter| {
        let unwound = panic::catch_unwind(AssertUnwindSafe(|| {
            running_span::<()>(
                Parent::current(),
                "job",
                SpanKind::Internal,
                Attributes::new(),
                ends_ok,
                |_span| panic!("a secret is in this message"),
            );
        }));

        let payload = unwound.expect_err("the body's panic reaches the caller");
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

// The strategy is telemetry's question about a result the body has already
// produced. If the question panics, it costs the span its status and never the
// caller the result.
#[test]
fn a_panicking_end_strategy_costs_a_status_and_never_the_result() {
    with_parenting(|exporter| {
        let rows = running_span(
            Parent::current(),
            "query",
            SpanKind::Internal,
            Attributes::new(),
            |_count: &i32| -> SpanStatus { panic!("the strategy is broken") },
            |_span| 7,
        );

        assert_eq!(rows, 7);
        assert_eq!(current_span_context(), None);
        assert_eq!(exported(exporter, "query").status, StatusCode::Unset);
        assert_eq!(drop_counts(), counts(0, 1, 0));

        // The same for a body that returns nothing.
        running_span(
            Parent::current(),
            "command",
            SpanKind::Internal,
            Attributes::new(),
            |_nothing: &()| -> SpanStatus { panic!("the strategy is broken") },
            |_span| {},
        );
        assert_eq!(exported(exporter, "command").status, StatusCode::Unset);
        assert_eq!(drop_counts().failed, 2);
    });
}

// --- Which span a log record or a metric point belongs to ---

#[test]
fn a_record_goes_under_the_parent_the_caller_names() {
    with_parenting(|exporter| {
        let outer = start_child("outer");
        let inner = start_child("inner");
        let remote = remote_context(0x0C, 0x0D, 0);

        record_log(
            Parent::current(),
            Severity::Info,
            "under the current span",
            Vec::new(),
        );
        record_log(
            Parent::span(outer.span()),
            Severity::Info,
            "under a given span",
            Vec::new(),
        );
        record_log(Parent::none(), Severity::Info, "under none", Vec::new());
        record_counter(Parent::current(), "by-current", 1, Vec::new());
        record_counter(Parent::span(outer.span()), "by-span", 1, Vec::new());
        record_histogram(Parent::remote(remote), "by-remote", 1.5, Vec::new());
        record_gauge(Parent::none(), "by-none", 2.5, Vec::new());

        let logs = exporter.logs();
        let under: Vec<Option<SpanContext>> = logs.iter().map(|log| log.span_context).collect();
        assert_eq!(
            under,
            [
                Some(inner.span().context()),
                Some(outer.span().context()),
                None
            ]
        );

        let metrics = exporter.metrics();
        let under: Vec<Option<SpanContext>> =
            metrics.iter().map(|point| point.span_context).collect();
        assert_eq!(
            under,
            [
                Some(inner.span().context()),
                Some(outer.span().context()),
                Some(remote),
                None
            ]
        );
    });
}

#[test]
fn a_record_with_no_span_anywhere_is_unparented_and_never_a_misuse() {
    with_parenting(|exporter| {
        let assertions = AssertionCapture::install();
        record_log(
            Parent::current(),
            Severity::Warn,
            "no span here",
            Vec::new(),
        );
        record_counter(Parent::current(), "no-span-here", 1, Vec::new());

        let logs = exporter.logs();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].span_context, None);
        assert_eq!(exporter.metrics()[0].span_context, None);
        assert!(
            assertions.failures().is_empty(),
            "{:?}",
            assertions.failures()
        );
        assert_eq!(dropped_count(), 0);
    });
}

// --- Telemetry never fails its caller, and never hides what it dropped ---

/// An exporter that refuses everything, as a broken backend does.
#[derive(Debug)]
struct PanickingExporter;

impl TelemetryExporter for PanickingExporter {
    fn export_spans(&self, _spans: &[Span]) {
        panic!("spans");
    }
    fn export_metrics(&self, _metrics: &[MetricPoint]) {
        panic!("metrics");
    }
    fn export_logs(&self, _logs: &[LogRecord]) {
        panic!("logs");
    }
    fn flush(&self) {}
    /// Holds nothing to drop; what it refuses, the pipeline counts.
    fn dropped(&self) -> u64 {
        0
    }
}

#[test]
fn an_exporter_that_panics_costs_a_record_and_a_count_not_the_caller() {
    let _serialized = pipeline_lock();
    let _pipeline = PipelineBuilder::new(Arc::new(PanickingExporter))
        .sampler(always_on_sampler())
        .epoch_source(clock_at(1))
        .id_source(counting_id_source())
        .install();

    // Reaching each next line is the "never fails the caller" half.
    drop(start_child("span"));
    record_log(Parent::current(), Severity::Info, "log", Vec::new());
    record_counter(Parent::current(), "counter", 1, Vec::new());
    running_span(
        Parent::current(),
        "ran",
        SpanKind::Internal,
        Attributes::new(),
        ends_ok,
        |_span| {},
    );

    assert_eq!(drop_counts(), counts(0, 4, 0));
    assert_eq!(dropped_count(), 4);
}

#[test]
fn a_clock_that_panics_costs_a_record_and_a_count_not_the_caller() {
    with_in_memory_configured(
        |builder| builder.epoch_source(EpochSource::new(|| panic!("the clock is broken"))),
        |exporter| {
            {
                let span = start_child("span");
                span.span().add_event("event", Attributes::new());
            }
            record_log(Parent::current(), Severity::Info, "log", Vec::new());
            record_gauge(Parent::current(), "gauge", 1.0, Vec::new());

            assert!(exporter.spans().is_empty(), "{:?}", exporter.spans());
            assert!(exporter.logs().is_empty(), "{:?}", exporter.logs());
            assert!(exporter.metrics().is_empty(), "{:?}", exporter.metrics());
            // The span, the log and the metric point. The event is not a fourth:
            // its span could not be timed, so it will never be exported, and
            // nothing is stored on or timed for such a span.
            assert_eq!(drop_counts(), counts(0, 3, 0));
        },
    );
}

// A span whose start could not be timed has no length to report. Exporting it
// would be a span of invented duration, so it is dropped — even though the
// clock works again by the time it ends — and it never tells a downstream
// service to sample a trace this process will not export.
#[test]
fn a_span_whose_start_could_not_be_timed_is_dropped_and_not_advertised_as_sampled() {
    let first_read = Arc::new(AtomicBool::new(true));
    let first = Arc::clone(&first_read);
    with_in_memory_configured(
        |builder| {
            builder.epoch_source(EpochSource::new(move || {
                assert!(
                    !first.swap(false, Ordering::SeqCst),
                    "the clock is not ready"
                );
                Epoch::from_millis(5)
            }))
        },
        |exporter| {
            {
                let untimed = start_child("untimed");
                assert!(!untimed.span().context().is_sampled());
            }
            {
                let timed = start_child("timed");
                assert!(timed.span().context().is_sampled());
            }

            assert_eq!(exported_names(exporter), ["timed"]);
            assert_eq!(drop_counts(), counts(0, 1, 0));
        },
    );
    assert!(!first_read.load(Ordering::SeqCst));
}

#[test]
fn with_no_pipeline_nothing_is_counted_because_there_is_nothing_to_count_on() {
    with_no_pipeline(|| {
        assert_eq!(dropped_count(), 0);
        drop(start_child("unrecorded"));
        record_log(Parent::current(), Severity::Info, "unrecorded", Vec::new());
        add_span_event("orphan", Attributes::new());
        assert_eq!(drop_counts(), DropCounts::default());
    });
}

// A span that started before any pipeline was installed has no start time, and
// — with no pipeline to draw ids from — the nil context, where C++ reads the
// operating system for ids. It is still current, so code that only reads the
// context is unaffected, and it is never exported, even to a pipeline installed
// before it ends; that pipeline counts nothing for a span it never saw start.
// What starts under it afterwards follows it: unexported, and counted.
#[test]
fn a_span_started_with_no_pipeline_is_never_exported() {
    let _serialized = pipeline_lock();
    shutdown_pipeline();
    let early = start_child("early");
    assert!(!early.span().context().trace_id.is_valid());
    assert!(!early.span().context().span_id.is_valid());
    assert!(!early.span().context().is_sampled());
    assert_eq!(current_span_context(), Some(early.span().context()));

    let exporter = Arc::new(InMemoryExporter::new());
    let _pipeline = PipelineBuilder::new(exporter.clone())
        .epoch_source(clock_at(9))
        .id_source(counting_id_source())
        .install();

    drop(start_child("child-of-early"));
    early
        .span()
        .add_event("after the install", Attributes::new());
    early.end(SpanStatus::ok());

    assert!(exporter.spans().is_empty(), "{:?}", exporter.spans());
    assert_eq!(drop_counts(), counts(1, 0, 0));
}

// One number for all three hid the difference between a sampler doing its job,
// a backend that is down, and a caller's mistake.
#[test]
fn drops_are_counted_by_reason() {
    with_sampling_by_name(&[("sampled-out", SamplingDecision::Drop)], |_| {
        let assertions = AssertionCapture::install();

        drop(start_child("sampled-out"));
        running_span(
            Parent::current(),
            "strategy-panics",
            SpanKind::Internal,
            Attributes::new(),
            |_result: &i32| -> SpanStatus { panic!("broken") },
            |_span| 1,
        );
        add_span_event("no-span-is-current", Attributes::new());

        let counted = drop_counts();
        assert_eq!(counted, counts(1, 1, 1));
        assert_eq!(counted.total(), 3);
        assert_eq!(dropped_count(), 3);
        assert_eq!(
            assertions.failures().len(),
            1,
            "only the misuse is reported"
        );
    });
}

// The clock is the composition root's code and may itself say something about
// the current span. It is read before the span's lock is taken, so that neither
// deadlocks nor is lost.
#[test]
fn a_clock_that_mutates_the_current_span_does_not_deadlock() {
    with_in_memory_configured(
        |builder| {
            builder.epoch_source(EpochSource::new(|| {
                set_span_attribute_if_any("seen-by-the-clock", true);
                Epoch::from_millis(1)
            }))
        },
        |exporter| {
            {
                let span = start_child("job");
                span.span().add_event("through-the-span", Attributes::new());
                add_span_event("through-the-context", Attributes::new());
            }

            let spans = exporter.spans();
            assert_eq!(spans.len(), 1);
            assert_eq!(spans[0].events.len(), 2);
            // One per clock read made while `job` was current: the two events.
            // The reads for its start and its end happen before it is current
            // and after it has stopped being.
            assert_eq!(spans[0].attributes.len(), 2);
        },
    );
}

// The assertion handler is the composition root's code too, and a misuse is
// reported with no span's lock and no borrow of the context held — so a handler
// may use telemetry: record a log and run a span of its own. (The C++ reports a
// mutation after a span ended, while another span is current; ending takes the
// handle here, so the misuse that can still be written is a mutation with no
// span current.)
#[test]
fn an_assertion_handler_that_uses_telemetry_does_not_deadlock() {
    with_parenting(|exporter| {
        let reports = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&reports);
        let previous = set_assertion_handler(Some(AssertionHandler::new(move |_failure| {
            counted.fetch_add(1, Ordering::SeqCst);
            set_span_attribute_if_any("a-misuse-was-reported", true);
            record_log(
                Parent::current(),
                Severity::Warn,
                "a misuse was reported",
                Vec::new(),
            );
            running_span(
                Parent::current(),
                "reported",
                SpanKind::Internal,
                Attributes::new(),
                ends_ok,
                |_span| {},
            );
        })));

        add_span_event("too-late", Attributes::new());
        drop(set_assertion_handler(previous));

        assert_eq!(reports.load(Ordering::SeqCst), 1);
        assert_eq!(exporter.logs().len(), 1);
        assert_eq!(exported_names(exporter), ["reported"]);
    });
}
