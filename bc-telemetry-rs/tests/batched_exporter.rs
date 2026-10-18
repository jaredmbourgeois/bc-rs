//! Batched asynchronous export.
//!
//! New surface — no ported cases. Nothing here sleeps as synchronization:
//! the drain is waited on by polling a condition with a generous deadline, and
//! the queue-full case is deterministic because the draining task cannot run
//! until the test awaits.
#![cfg(feature = "batched")]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use bc_telemetry::{BatchedExporter, InMemoryExporter, Span, TelemetryExporter};

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a tokio runtime")
}

/// An exporter that records how often it was flushed.
///
/// Shared by both shutdown tests: delivery is draining *and* flushing, and the
/// two ways a `BatchedExporter` can be built each have to do both.
#[derive(Debug)]
struct CountsFlushes {
    inner: Arc<InMemoryExporter>,
    flushes: AtomicUsize,
}

impl CountsFlushes {
    fn new() -> Self {
        Self {
            inner: Arc::new(InMemoryExporter::new()),
            flushes: AtomicUsize::new(0),
        }
    }

    fn flushes(&self) -> usize {
        self.flushes.load(Ordering::Relaxed)
    }
}

impl TelemetryExporter for CountsFlushes {
    fn export_spans(&self, spans: &[Span]) {
        self.inner.export_spans(spans);
    }
    fn export_metrics(&self, metrics: &[bc_telemetry::MetricPoint]) {
        self.inner.export_metrics(metrics);
    }
    fn export_logs(&self, logs: &[bc_telemetry::LogRecord]) {
        self.inner.export_logs(logs);
    }
    fn flush(&self) {
        self.flushes.fetch_add(1, Ordering::Relaxed);
    }
    fn dropped(&self) -> u64 {
        self.inner.dropped()
    }
}

#[test]
fn shutdown_outside_a_runtime_still_flushes_the_inner_exporter() {
    // Outside a runtime there is no queue to drain, but `inner` may still be
    // holding buffered records, so a shutdown that returns without flushing
    // loses exactly the data it was called to save. The runtime path was
    // corrected for this; the fallback kept the defect.
    let counting = Arc::new(CountsFlushes::new());
    let batched = BatchedExporter::new(counting.clone(), 32);

    runtime().block_on(batched.shutdown());

    assert_eq!(
        counting.flushes(),
        1,
        "shutdown outside a runtime left the inner exporter unflushed"
    );
}

fn a_span(name: &str) -> Span {
    Span {
        name: name.to_owned(),
        ..Span::default()
    }
}

#[test]
fn queued_records_reach_the_inner_exporter() {
    let inner = Arc::new(InMemoryExporter::new());
    runtime().block_on(async {
        let batched = BatchedExporter::new(inner.clone(), 32);
        batched.export_spans(&[a_span("one")]);
        batched.export_spans(&[a_span("two")]);

        // Poll until the drain has caught up, with a deadline generous enough
        // that a loaded machine does not fail it.
        let deadline = Instant::now() + Duration::from_secs(10);
        while inner.spans().len() < 2 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        let names: Vec<String> = inner.spans().into_iter().map(|span| span.name).collect();
        assert_eq!(names, ["one", "two"]);
        assert_eq!(batched.dropped(), 0);
    });
}

#[test]
fn a_full_queue_drops_and_counts_rather_than_blocking() {
    // Back-pressure would make instrumentation slow down the work it measures,
    // which is the one thing telemetry must never do. Deterministic: the
    // draining task cannot be polled before the first await below, so the queue
    // genuinely fills.
    let inner = Arc::new(InMemoryExporter::new());
    runtime().block_on(async {
        let batched = BatchedExporter::new(inner.clone(), 1);
        for index in 0..64 {
            batched.export_spans(&[a_span(&format!("span-{index}"))]);
        }
        assert!(
            batched.dropped() > 0,
            "a bounded queue accepted 64 batches without dropping"
        );
        // `dropped()` now also carries the inner exporter's own count, so this
        // is a real relationship rather than the tautology it used to be:
        // InMemoryExporter never drops, so the two agree only for that reason.
        assert_eq!(batched.dropped(), batched.dropped_count() + inner.dropped());
    });
}

#[test]
fn an_exporter_that_panics_does_not_take_the_drain_down() {
    // Telemetry never fails the caller, and by the time an export runs the
    // caller is gone anyway — so a panicking exporter must not stop the ones
    // queued behind it.
    #[derive(Debug)]
    struct PanicsOnce(Arc<InMemoryExporter>, std::sync::atomic::AtomicBool);

    impl TelemetryExporter for PanicsOnce {
        fn export_spans(&self, spans: &[Span]) {
            let is_first = !self.1.swap(true, std::sync::atomic::Ordering::Relaxed);
            assert!(!is_first, "the first batch explodes");
            self.0.export_spans(spans);
        }
        fn export_metrics(&self, _: &[bc_telemetry::MetricPoint]) {}
        fn export_logs(&self, _: &[bc_telemetry::LogRecord]) {}
        fn flush(&self) {}
        /// It drops nothing itself: the batch its panic lost is the drain's
        /// to count, and the test below holds the drain to it.
        fn dropped(&self) -> u64 {
            0
        }
    }

    let inner = Arc::new(InMemoryExporter::new());
    let panicking = Arc::new(PanicsOnce(
        inner.clone(),
        std::sync::atomic::AtomicBool::new(false),
    ));

    runtime().block_on(async {
        let batched = BatchedExporter::new(panicking, 32);
        batched.export_spans(&[a_span("explodes")]);
        batched.export_spans(&[a_span("survives")]);

        let deadline = Instant::now() + Duration::from_secs(10);
        while inner.spans().is_empty() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let names: Vec<String> = inner.spans().into_iter().map(|span| span.name).collect();
        assert_eq!(names, ["survives"], "the drain kept going after a panic");
        // The panic's batch is lost, and counted by the drain that contained it.
        assert_eq!(batched.dropped(), 1, "the panicked batch went uncounted");
    });
}

#[test]
fn shutdown_drains_what_is_still_queued() {
    // The failure this pins: `flush()` only *queues* a request and returns, so a
    // process that exited after calling it lost everything still in the backlog
    // while `dropped()` reported zero — a silent loss at the one moment the
    // count matters most. `shutdown()` is the guarantee.
    let inner = Arc::new(InMemoryExporter::new());
    runtime().block_on(async {
        let batched = BatchedExporter::new(inner.clone(), 1024);
        for index in 0..256 {
            batched.export_spans(&[a_span(&format!("span-{index}"))]);
        }
        batched.shutdown().await;
    });
    assert_eq!(inner.spans().len(), 256);
    assert_eq!(inner.dropped(), 0);
}

#[test]
fn constructing_outside_a_runtime_drops_and_counts_rather_than_writing_inline() {
    // `tokio::spawn` panics with "there is no reactor running" outside a
    // runtime, and a constructor in this crate must not be the one place that
    // aborts a caller's program. Nor may it write on the caller's thread: a
    // writer never makes a request wait on the database. So with no task to
    // drain into `inner`, every record is dropped and counted.
    let inner = Arc::new(InMemoryExporter::new());
    let batched = BatchedExporter::new(inner.clone(), 8);
    batched.export_spans(&[a_span("one"), a_span("two")]);
    batched.flush();
    assert!(
        inner.spans().is_empty(),
        "a record was written on the caller's thread"
    );
    assert_eq!(batched.dropped(), 2);
}

#[test]
fn shutdown_flushes_the_inner_exporter_after_draining_it() {
    // Draining is only half of delivery. An exporter that buffers — a file
    // exporter's writer, a batching client — still holds the last records after
    // the queue is empty, so a shutdown that hands them over and stops loses
    // exactly the data it was called to save. The sentinel maps to a flush and
    // ends the loop afterwards; ending it first left the flush arm unreachable.
    let counting = Arc::new(CountsFlushes::new());
    let inner = counting.inner.clone();
    runtime().block_on(async {
        let batched = BatchedExporter::new(counting.clone(), 1024);
        batched.export_spans(&[a_span("last")]);
        batched.shutdown().await;
    });

    assert_eq!(inner.spans().len(), 1, "the queued span never arrived");
    assert_eq!(
        counting.flushes.load(std::sync::atomic::Ordering::Relaxed),
        1,
        "shutdown drained the queue but left the inner exporter unflushed"
    );
}
