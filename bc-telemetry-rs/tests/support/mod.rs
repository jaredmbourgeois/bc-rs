//! Shared fixtures for the ported suite.
//!
//! The pipeline is process-global, exactly as it is in the C++. gtest runs cases
//! serially so the original needs no arrangement; Rust's default harness runs
//! them on threads in one process, so an install here would race a shutdown
//! there. `cargo nextest` — which `scripts/verify.sh` prefers — gives each case
//! its own process and makes the point moot, but the plain `cargo test`
//! fallback does not, and a suite that only passes under one runner is a suite
//! that will fail confusingly under the other.
//!
//! So every case that installs a pipeline takes this lock. It is not a sleep or
//! a guess: it is the actual invariant, which is that one process has one
//! pipeline.
#![allow(dead_code, unreachable_pub)]
// This module is compiled into each integration-test binary separately, and
// each uses a different subset of it. Without these, every binary warns about
// the helpers the *other* binaries need.

use std::panic::Location;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use bc_core::{AssertionFailure, AssertionHandler, set_assertion_handler};
use bc_date_time::{Epoch, EpochSource, fixed_epoch_source};
use bc_telemetry::{IdSource, InMemoryExporter, PipelineBuilder, Sampler, Span};

static PIPELINE_LOCK: Mutex<()> = Mutex::new(());

/// Takes the one-pipeline-per-process lock directly.
///
/// For a case that installs a pipeline itself rather than through the helpers
/// below — it still may not run beside one that does.
pub fn pipeline_lock() -> MutexGuard<'static, ()> {
    lock()
}

fn lock() -> MutexGuard<'static, ()> {
    // A panicking case poisons the lock; the next one still needs to run, and
    // the data guarded is `()`, so there is nothing that could be inconsistent.
    PIPELINE_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

/// An [`IdSource`] whose ids are deterministic *and* distinct.
///
/// `fixed_id_source` cannot serve here: it hands every span the same id, so a
/// case asserting that a parent and child differ would fail against correct
/// code. This counts instead, which keeps ids reproducible without making them
/// equal.
#[must_use]
pub fn counting_id_source() -> IdSource {
    let counter = Arc::new(AtomicU64::new(0));
    IdSource::new(move |buffer| {
        let value = counter.fetch_add(1, Ordering::Relaxed) + 1;
        buffer.fill(0);
        let encoded = value.to_be_bytes();
        let take = encoded.len().min(buffer.len());
        let start = buffer.len() - take;
        buffer[start..].copy_from_slice(&encoded[encoded.len() - take..]);
    })
}

/// A clock that starts at `millis` and advances one millisecond per read, so
/// every timestamp in a case is exact and every span has a length.
#[must_use]
pub fn ticking_clock_from(millis: i64) -> EpochSource {
    let next = Arc::new(AtomicI64::new(millis));
    EpochSource::new(move || Epoch::from_millis(next.fetch_add(1, Ordering::Relaxed)))
}

/// The one exported span named `name`. Fails the case when there is not
/// exactly one.
#[track_caller]
pub fn exported(exporter: &InMemoryExporter, name: &str) -> Span {
    let mut matches: Vec<Span> = exporter
        .spans()
        .into_iter()
        .filter(|span| span.name == name)
        .collect();
    assert_eq!(matches.len(), 1, "spans named {name}: {matches:?}");
    matches.remove(0)
}

/// The names of the exported spans, in the order they ended.
#[must_use]
pub fn exported_names(exporter: &InMemoryExporter) -> Vec<String> {
    exporter.spans().into_iter().map(|span| span.name).collect()
}

/// One reported assertion failure, copied out of what the handler only borrows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reported {
    pub expectation: String,
    pub detail: String,
    pub location: &'static Location<'static>,
}

/// Captures every assertion failure reported while it lives, and restores the
/// handler it displaced when it goes — `bourgeois_concern`'s
/// `bc::test::AssertionCapture`. The handler is process-wide, like the
/// pipeline, so a case installs one only under the pipeline lock.
pub struct AssertionCapture {
    captured: Arc<Mutex<Vec<Reported>>>,
    previous: Option<AssertionHandler>,
}

impl AssertionCapture {
    #[must_use]
    pub fn install() -> Self {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&captured);
        let previous = set_assertion_handler(Some(AssertionHandler::new(
            move |failure: &AssertionFailure<'_>| {
                sink.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(Reported {
                        expectation: failure.expectation.to_owned(),
                        detail: failure.detail.to_owned(),
                        location: failure.location,
                    });
            },
        )));
        Self { captured, previous }
    }

    #[must_use]
    pub fn failures(&self) -> Vec<Reported> {
        self.captured
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Drop for AssertionCapture {
    fn drop(&mut self) {
        // The displaced handler comes back; this capture's own is what the
        // replacement hands over, and it is done with.
        drop(set_assertion_handler(self.previous.take()));
    }
}

/// A clock pinned at `millis`, so every timestamp in a case is exact.
#[must_use]
pub fn clock_at(millis: i64) -> EpochSource {
    fixed_epoch_source(Epoch::from_millis(millis))
}

/// Runs `body` with a freshly installed in-memory pipeline, then tears it down.
///
/// The pipeline is uninstalled even when `body` panics, because the scope is
/// dropped during unwinding — which is the same guarantee `PipelineScope` gives
/// a real program on an early return.
pub fn with_in_memory<R>(body: impl FnOnce(&Arc<InMemoryExporter>) -> R) -> R {
    with_in_memory_configured(|builder| builder, body)
}

/// As [`with_in_memory`], letting a case adjust the builder first.
pub fn with_in_memory_configured<R>(
    configure: impl FnOnce(PipelineBuilder) -> PipelineBuilder,
    body: impl FnOnce(&Arc<InMemoryExporter>) -> R,
) -> R {
    let _guard = lock();
    let exporter = Arc::new(InMemoryExporter::new());
    let builder = PipelineBuilder::new(exporter.clone())
        .epoch_source(clock_at(1_000))
        .id_source(counting_id_source());
    let _pipeline = configure(builder).install();
    body(&exporter)
}

/// As [`with_in_memory`], with a chosen sampler.
pub fn with_sampler<R>(sampler: Sampler, body: impl FnOnce(&Arc<InMemoryExporter>) -> R) -> R {
    with_in_memory_configured(|builder| builder.sampler(sampler), body)
}

/// Runs `body` with **no** pipeline installed, holding the lock so a concurrent
/// case cannot install one underneath it.
pub fn with_no_pipeline<R>(body: impl FnOnce() -> R) -> R {
    let _guard = lock();
    bc_telemetry::shutdown_pipeline();
    body()
}

/// Runs `body` with no pipeline installed, while still holding the exporter that
/// *was* installed a moment ago.
///
/// The point is assertability. A case that only calls a recording function with
/// nothing installed can fail solely by panicking, which makes "is a no-op" a
/// claim about the absence of a crash rather than about where the record went.
/// Keeping the uninstalled exporter alive lets the case observe that it stayed
/// empty.
pub fn with_uninstalled_exporter<R>(body: impl FnOnce(&Arc<InMemoryExporter>) -> R) -> R {
    let _guard = lock();
    let exporter = Arc::new(InMemoryExporter::new());
    {
        let _pipeline = PipelineBuilder::new(exporter.clone()).install();
    }
    body(&exporter)
}
