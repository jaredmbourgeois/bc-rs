//! Ported from `libs/platform/telemetry/test/pipeline_test.cpp`.

mod support;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use bc_telemetry::{
    Attributes, InMemoryExporter, LogRecord, MetricPoint, Parent, PipelineBuilder, Sampler,
    SamplingDecision, Severity, Span, SpanGuard, SpanKind, TelemetryExporter, TraceId, drop_counts,
    fixed_id_source, installed_pipeline, probability_sampler, record_counter, record_gauge,
    record_log, set_span_attribute, shutdown_pipeline, start_span,
};

use support::{
    exported_names, pipeline_lock, with_in_memory, with_in_memory_configured, with_no_pipeline,
    with_sampler,
};

/// An internal span under the current one, with no attributes.
fn start_child(name: &str) -> SpanGuard {
    start_span(
        Parent::current(),
        name,
        SpanKind::Internal,
        Attributes::new(),
    )
}

#[test]
fn no_pipeline_spans_are_no_op() {
    with_no_pipeline(|| {
        // The whole point of the rule: instrumentation runs with nothing
        // installed and neither fails nor panics.
        {
            let _orphan = start_child("orphan");
        }
        assert!(installed_pipeline().is_none());
    });
}

#[test]
fn install_and_shutdown() {
    with_in_memory(|exporter| {
        assert!(installed_pipeline().is_some());
        drop(start_child("recorded"));
        assert_eq!(exporter.spans().len(), 1);

        shutdown_pipeline();
        assert!(installed_pipeline().is_none());

        drop(start_child("after shutdown"));
        assert_eq!(exporter.spans().len(), 1, "nothing more was exported");
    });
}

#[test]
fn probability_sampler_drops_spans() {
    with_sampler(probability_sampler(0.0), |exporter| {
        drop(start_child("dropped"));
        assert!(exporter.spans().is_empty(), "{:?}", exporter.spans());
    });
}

#[test]
fn probability_sampler_accepts_all() {
    with_sampler(probability_sampler(1.0), |exporter| {
        drop(start_child("kept"));
        assert_eq!(exporter.spans().len(), 1);
    });
}

#[test]
fn a_ratio_of_one_samples_every_trace_id() {
    // The C++ clamps this endpoint to avoid undefined behavior; Rust's
    // saturating cast removes the hazard, so this pins the *behavior* rather
    // than the workaround.
    let sampler = probability_sampler(1.0);
    for byte in [0x00_u8, 0x7f, 0x80, 0xff] {
        let trace_id = TraceId { bytes: [byte; 16] };
        assert_eq!(
            sampler.decide(&trace_id, "any"),
            SamplingDecision::RecordAndSample,
            "byte {byte:#x}"
        );
    }
}

#[test]
fn always_on_sampler_is_the_default() {
    let _serialized = pipeline_lock();
    let exporter = Arc::new(InMemoryExporter::new());
    // Built without naming a sampler at all, then actually installed: the claim
    // is about `PipelineBuilder::new`'s default, so the pipeline has to be in
    // force and a span has to run through it.
    {
        let _pipeline = PipelineBuilder::new(exporter.clone())
            .id_source(fixed_id_source(1))
            .install();
        drop(start_child("sampled-by-default"));
    }
    assert_eq!(exporter.spans().len(), 1);
    assert!(exporter.spans()[0].context.is_sampled());
}

#[test]
fn a_drop_sampler_records_nothing_and_counts_the_drop() {
    // The discriminating half of the case above: same shape, opposite sampler.
    // Without this, "the default samples" cannot be told apart from "every
    // sampler samples".
    let _serialized = pipeline_lock();
    let exporter = Arc::new(InMemoryExporter::new());
    let unsampled = {
        let _pipeline = PipelineBuilder::new(exporter.clone())
            .sampler(Sampler::new(|_, _| SamplingDecision::Drop))
            .install();
        drop(start_child("dropped"));
        drop_counts().unsampled
    };
    assert!(exporter.spans().is_empty(), "{:?}", exporter.spans());
    // Dropping is permitted, hiding the drop is not.
    assert_eq!(unsampled, 1);
}

#[test]
fn scope_shuts_down_on_exit() {
    let _serialized = pipeline_lock();
    let exporter = Arc::new(InMemoryExporter::new());
    {
        let _scope = PipelineBuilder::new(exporter.clone()).install();
        assert!(installed_pipeline().is_some());
    }
    assert!(installed_pipeline().is_none());
}

#[test]
fn scope_shutdown_is_idempotent() {
    let _serialized = pipeline_lock();
    let exporter = Arc::new(InMemoryExporter::new());
    {
        let _scope = PipelineBuilder::new(exporter).install();
        shutdown_pipeline();
        shutdown_pipeline();
        // The scope's own drop runs a third time on the way out of this block.
    }
    assert!(installed_pipeline().is_none());
}

/// Drops one record in three and says so, as a backend that is behind does.
#[derive(Debug, Default)]
struct DropsSome {
    seen: AtomicUsize,
}

impl TelemetryExporter for DropsSome {
    fn export_spans(&self, _spans: &[Span]) {
        self.seen.fetch_add(1, Ordering::SeqCst);
    }
    fn export_metrics(&self, _metrics: &[MetricPoint]) {}
    fn export_logs(&self, _logs: &[LogRecord]) {}
    fn flush(&self) {}
    fn dropped(&self) -> u64 {
        u64::try_from(self.seen.load(Ordering::SeqCst) / 3).unwrap_or(u64::MAX)
    }
}

#[test]
fn a_pipelines_dropped_count_is_its_own_drops_and_its_exporters() {
    // A drop is never hidden: a reader asking how much is missing gets one
    // number covering both places a record can be lost — what the pipeline
    // never handed on, and what the exporter could not write.
    let _serialized = pipeline_lock();
    let exporter = Arc::new(DropsSome::default());
    let _pipeline = PipelineBuilder::new(exporter.clone())
        .sampler(Sampler::new(|_, name| {
            if name == "sampled-out" {
                SamplingDecision::Drop
            } else {
                SamplingDecision::RecordAndSample
            }
        }))
        .install();
    drop(start_child("sampled-out"));
    for _ in 0..6 {
        drop(start_child("exported"));
    }

    let pipeline = installed_pipeline().expect("installed above");
    assert_eq!(pipeline.drop_counts().total(), 1);
    assert_eq!(exporter.dropped(), 2);
    assert_eq!(pipeline.dropped(), 3);
}

// --- Install and shutdown run caller-supplied code outside the slot's lock ---

/// A test exporter that records how many times it was flushed.
#[derive(Debug, Default)]
struct FlushCounting {
    flushes: AtomicUsize,
}

impl TelemetryExporter for FlushCounting {
    fn export_spans(&self, _spans: &[Span]) {}
    fn export_metrics(&self, _metrics: &[MetricPoint]) {}
    fn export_logs(&self, _logs: &[LogRecord]) {}
    fn flush(&self) {
        self.flushes.fetch_add(1, Ordering::SeqCst);
    }
    /// Discards everything, and says so.
    fn dropped(&self) -> u64 {
        0
    }
}

#[test]
fn installing_over_a_live_pipeline_flushes_the_displaced_exporter() {
    // Whatever the displaced exporter had buffered used to be discarded
    // uncounted: `install` overwrote the slot and never flushed what it evicted.
    let _serialized = pipeline_lock();
    let displaced = Arc::new(FlushCounting::default());
    let _outer = PipelineBuilder::new(displaced.clone()).install();
    assert_eq!(displaced.flushes.load(Ordering::SeqCst), 0);

    let _inner = PipelineBuilder::new(Arc::new(InMemoryExporter::new())).install();
    assert_eq!(displaced.flushes.load(Ordering::SeqCst), 1);
}

/// Records a log from its `Drop`, then says it was destroyed. A drop under the
/// slot's lock would make that `record_log` wait on the lock it is under.
#[derive(Debug)]
struct RecordsWhenDestroyed {
    destroyed: mpsc::Sender<()>,
}

impl Drop for RecordsWhenDestroyed {
    fn drop(&mut self) {
        record_log(
            Parent::current(),
            Severity::Info,
            "exporter destroyed",
            Vec::new(),
        );
        // The receiver goes only when the case has already failed by timeout.
        let _ = self.destroyed.send(());
    }
}

impl TelemetryExporter for RecordsWhenDestroyed {
    fn export_spans(&self, _spans: &[Span]) {}
    fn export_metrics(&self, _metrics: &[MetricPoint]) {}
    fn export_logs(&self, _logs: &[LogRecord]) {}
    fn flush(&self) {}
    /// Discards everything, and says so.
    fn dropped(&self) -> u64 {
        0
    }
}

#[test]
fn a_displaced_exporter_is_destroyed_outside_the_lock_and_may_record() {
    // Rewritten from `DisplacedExporterIsDestroyedOutsideTheLockAndMayRecord`.
    // In C++ the install that displaces a pipeline destroys it; here the new
    // scope keeps the displaced one to restore, so it is destroyed when the
    // last scope holding it ends. What is asserted is the same: wherever that
    // happens, the exporter's drop may record. The scopes run on a thread of
    // their own so a regression is a hang there, named at two seconds, rather
    // than a hang in this body with no diagnosis.
    let _serialized = pipeline_lock();
    let (destroyed, was_destroyed) = mpsc::channel();
    let replacer = thread::spawn(move || {
        let _first = PipelineBuilder::new(Arc::new(RecordsWhenDestroyed { destroyed })).install();
        let _second = PipelineBuilder::new(Arc::new(InMemoryExporter::new())).install();
        // `_second` hands the first pipeline back, then `_first` takes it out
        // of the slot, and the last reference to its exporter goes.
    });
    assert!(
        was_destroyed.recv_timeout(Duration::from_secs(2)).is_ok(),
        "the displaced exporter was not destroyed within 2 s — a drop held the lock"
    );
    replacer.join().expect("the replacing thread did not panic");
    assert!(installed_pipeline().is_none());
}

/// Records a log from `flush` — the shutdown path's caller-supplied code.
#[derive(Debug)]
struct RecordsWhenFlushed;

impl TelemetryExporter for RecordsWhenFlushed {
    fn export_spans(&self, _spans: &[Span]) {}
    fn export_metrics(&self, _metrics: &[MetricPoint]) {}
    fn export_logs(&self, _logs: &[LogRecord]) {}
    fn flush(&self) {
        record_log(Parent::current(), Severity::Info, "flushing", Vec::new());
    }
    /// Discards everything, and says so.
    fn dropped(&self) -> u64 {
        0
    }
}

#[test]
fn a_shutdown_flush_runs_outside_the_lock_so_a_recorder_is_not_stalled() {
    // `flush` calls `record_log`, which reads the slot. Under a shutdown that
    // flushed while holding the slot's write lock, that read would never return.
    let _serialized = pipeline_lock();
    let _scope = PipelineBuilder::new(Arc::new(RecordsWhenFlushed)).install();
    shutdown_pipeline();
    assert!(installed_pipeline().is_none());
}

#[test]
fn a_nested_scope_hands_the_pipeline_back_rather_than_clearing_it() {
    // `PipelineScope::drop` used to call the unconditional `shutdown_pipeline`,
    // so an inner scope's exit left the process with *no* pipeline while the
    // outer scope was still alive and still nominally owning one.
    let _serialized = pipeline_lock();
    let outer_exporter = Arc::new(InMemoryExporter::new());
    let _outer = PipelineBuilder::new(outer_exporter.clone()).install();
    {
        let inner_exporter = Arc::new(InMemoryExporter::new());
        let _inner = PipelineBuilder::new(inner_exporter.clone()).install();
        drop(start_child("inner"));
        assert_eq!(exported_names(&inner_exporter), ["inner"]);
        assert!(
            outer_exporter.spans().is_empty(),
            "{:?}",
            outer_exporter.spans()
        );
    }
    // The outer pipeline is back in force, so this span reaches its exporter.
    drop(start_child("outer"));
    assert_eq!(exported_names(&outer_exporter), ["outer"]);
}

#[test]
fn a_scope_does_not_resurrect_a_pipeline_that_was_explicitly_shut_down() {
    // The scope's job is to unwind *its own* install. An explicit shutdown in
    // between is a deliberate act, and a scope that restored what it displaced
    // regardless would hand the process back a pipeline it had already taken
    // down — quietly, on the way out of an unrelated block.
    let _serialized = pipeline_lock();
    let outer_exporter = Arc::new(InMemoryExporter::new());
    let _outer = PipelineBuilder::new(outer_exporter).install();
    {
        let _inner = PipelineBuilder::new(Arc::new(InMemoryExporter::new())).install();
        shutdown_pipeline();
        assert!(installed_pipeline().is_none());
    }
    assert!(
        installed_pipeline().is_none(),
        "the inner scope put the outer pipeline back after an explicit shutdown"
    );
}

// --- Concurrency ---

#[test]
fn shutdown_does_not_race_in_flight_recording() {
    // A call in flight holds an `Arc` to the pipeline it read, so a concurrent
    // install or shutdown on another thread cannot free it mid-record. Reaching
    // the end with every thread joined and none panicked is the assertion: four
    // recorders run every kind of call while this thread installs and shuts
    // down two hundred times. No sleeps: bounded churn and joins.
    let _serialized = pipeline_lock();
    let stop = AtomicBool::new(false);
    thread::scope(|scope| {
        let recorders: Vec<_> = (0..4)
            .map(|_| {
                scope.spawn(|| {
                    while !stop.load(Ordering::Relaxed) {
                        {
                            let _span = start_child("span");
                            set_span_attribute("key", 1_i64);
                        }
                        record_counter(Parent::current(), "counter", 1, Vec::new());
                        record_log(Parent::current(), Severity::Info, "log", Vec::new());
                    }
                })
            })
            .collect();

        for _ in 0..200 {
            let _churn = PipelineBuilder::new(Arc::new(InMemoryExporter::new())).install();
            shutdown_pipeline();
        }

        stop.store(true, Ordering::Relaxed);
        for recorder in recorders {
            recorder.join().expect("no recorder panicked");
        }
    });
    assert!(installed_pipeline().is_none());
}

#[test]
fn every_concurrently_recorded_span_reaches_the_installed_exporter() {
    // The case above proves nothing is FREED under churn; this one proves
    // nothing is LOST when the pipeline stays put. Four recorders each end a
    // hundred spans against one exporter while four more read the pipeline;
    // the joins are the synchronization.
    let _serialized = pipeline_lock();
    let exporter = Arc::new(InMemoryExporter::new());
    let _scope = PipelineBuilder::new(exporter.clone())
        .id_source(fixed_id_source(3))
        .install();

    let stop = AtomicBool::new(false);
    thread::scope(|scope| {
        let recorders: Vec<_> = (0..4)
            .map(|_| {
                scope.spawn(|| {
                    for _ in 0..100 {
                        drop(start_child("concurrent"));
                    }
                })
            })
            .collect();
        for _ in 0..4 {
            scope.spawn(|| {
                while !stop.load(Ordering::Relaxed) {
                    // Hold a snapshot across a read, which is what an export in
                    // flight does.
                    if let Some(pipeline) = installed_pipeline() {
                        let _ = pipeline.now();
                    }
                }
            });
        }
        for recorder in recorders {
            recorder.join().expect("no recorder panicked");
        }
        stop.store(true, Ordering::Relaxed);
    });
    assert_eq!(exporter.spans().len(), 4 * 100);
}

// --- Observation ids: new surface, no ported cases -----------------------------

/// Every point and log a pipeline records carries an id under one producer —
/// drawn from the pipeline's id source when it was installed — counted from
/// one across both kinds, so a writer can store each once.
#[test]
fn points_and_logs_are_counted_from_one_under_one_producer() {
    with_in_memory(|exporter| {
        record_counter(Parent::current(), "first", 1, Vec::new());
        record_log(Parent::current(), Severity::Info, "second", Vec::new());
        record_gauge(Parent::current(), "third", 2.0, Vec::new());

        let metrics = exporter.metrics();
        let logs = exporter.logs();
        let producer = metrics[0].observation_id.producer;
        assert!(producer.is_valid());
        assert_eq!(logs[0].observation_id.producer, producer);
        assert_eq!(metrics[1].observation_id.producer, producer);
        assert_eq!(
            [
                metrics[0].observation_id.sequence,
                logs[0].observation_id.sequence,
                metrics[1].observation_id.sequence,
            ],
            [1, 2, 3]
        );
    });
}

/// A pipeline whose id source gives only zeros has no producer, so it can
/// issue no id: what it records is dropped and counted rather than exported
/// under an id no writer could store once.
#[test]
fn a_pipeline_that_can_issue_no_id_drops_points_and_logs_and_counts_them() {
    with_in_memory_configured(
        |builder| builder.id_source(fixed_id_source(0)),
        |exporter| {
            record_counter(Parent::current(), "requests", 1, Vec::new());
            record_log(Parent::current(), Severity::Info, "hello", Vec::new());
            assert!(exporter.metrics().is_empty(), "{:?}", exporter.metrics());
            assert!(exporter.logs().is_empty(), "{:?}", exporter.logs());
            assert_eq!(drop_counts().identity, 2);
            assert_eq!(drop_counts().total(), 2);
        },
    );
}
