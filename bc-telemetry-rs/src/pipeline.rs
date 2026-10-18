//! The process-global pipeline: exporter, sampler, clock, and id source.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use bc_date_time::{Epoch, EpochSource, system_epoch_source};

use crate::exporter::{Sampler, TelemetryExporter};
use crate::id_source::{IdSource, ObservationSequence};
use crate::types::{ObservationId, TraceId};

/// Why something the process tried to record is absent from what the exporter
/// holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropReason {
    /// A span the sampler dropped, or whose parent is not exported.
    Unsampled,
    /// Caller-supplied code panicked: the exporter, the clock, the sampler, the
    /// id source, an end strategy.
    Failed,
    /// A misuse of the span API, one per report.
    Misuse,
    /// A point or a log the pipeline could issue no observation id for, which
    /// no writer could store once. New surface beside the C++, with the id.
    Identity,
}

/// What the installed pipeline has dropped, by reason. One number for all of
/// them hid the difference between a sampler doing its job and an exporter
/// that is down.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DropCounts {
    /// See [`DropReason::Unsampled`].
    pub unsampled: u64,
    /// See [`DropReason::Failed`].
    pub failed: u64,
    /// See [`DropReason::Misuse`].
    pub misuse: u64,
    /// See [`DropReason::Identity`].
    pub identity: u64,
}

impl DropCounts {
    /// Every drop, whatever its reason.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.unsampled
            .saturating_add(self.failed)
            .saturating_add(self.misuse)
            .saturating_add(self.identity)
    }
}

/// Everything a recording call needs, installed as one unit.
#[derive(Debug)]
pub struct Pipeline {
    exporter: Arc<dyn TelemetryExporter>,
    sampler: Sampler,
    epoch_source: EpochSource,
    id_source: IdSource,
    observations: ObservationSequence,
    unsampled: AtomicU64,
    failed: AtomicU64,
    misuse: AtomicU64,
    identity: AtomicU64,
}

impl Pipeline {
    /// The installed exporter.
    #[must_use]
    pub fn exporter(&self) -> &dyn TelemetryExporter {
        self.exporter.as_ref()
    }

    /// The installed sampler.
    #[must_use]
    pub const fn sampler(&self) -> &Sampler {
        &self.sampler
    }

    /// The installed id source.
    #[must_use]
    pub const fn id_source(&self) -> &IdSource {
        &self.id_source
    }

    /// The id of the next metric point or log record this pipeline records, or
    /// `None` when it can issue none: its id source gave an all-zero producer,
    /// or every sequence has been issued.
    #[must_use]
    pub fn next_observation(&self) -> Option<ObservationId> {
        self.observations.next()
    }

    /// Reads the installed clock.
    #[must_use]
    pub fn now(&self) -> Epoch {
        self.epoch_source.now()
    }

    /// Records that one telemetry item was dropped rather than exported, and
    /// why.
    pub fn record_drop(&self, reason: DropReason) {
        let counter = match reason {
            DropReason::Unsampled => &self.unsampled,
            DropReason::Failed => &self.failed,
            DropReason::Misuse => &self.misuse,
            DropReason::Identity => &self.identity,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// What this pipeline itself has dropped, by reason.
    #[must_use]
    pub fn drop_counts(&self) -> DropCounts {
        DropCounts {
            unsampled: self.unsampled.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
            misuse: self.misuse.load(Ordering::Relaxed),
            identity: self.identity.load(Ordering::Relaxed),
        }
    }

    /// How many items this pipeline knows were dropped.
    ///
    /// The sum of what the pipeline itself dropped, for every reason, and what
    /// its exporter reports. Dropping is permitted, hiding the drop is not — so
    /// a reader can always ask, and a gap is visible rather than seamless.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.drop_counts()
            .total()
            .saturating_add(self.exporter.dropped())
    }
}

/// How a pipeline is assembled.
///
/// A builder rather than a four-argument `install`: the C++ takes defaulted
/// arguments, which Rust has no form for, and three of the four parts have a
/// sensible default that a caller usually wants.
#[derive(Debug)]
pub struct PipelineBuilder {
    exporter: Arc<dyn TelemetryExporter>,
    sampler: Sampler,
    epoch_source: Option<EpochSource>,
    id_source: IdSource,
}

impl PipelineBuilder {
    /// Starts from an exporter; everything else defaults.
    #[must_use]
    pub fn new(exporter: Arc<dyn TelemetryExporter>) -> Self {
        Self {
            exporter,
            sampler: Sampler::default(),
            epoch_source: None,
            id_source: IdSource::default(),
        }
    }

    /// Uses `sampler` instead of [`crate::always_on_sampler`].
    #[must_use]
    pub fn sampler(mut self, sampler: Sampler) -> Self {
        self.sampler = sampler;
        self
    }

    /// Uses `epoch_source` instead of the system clock.
    #[must_use]
    pub fn epoch_source(mut self, epoch_source: EpochSource) -> Self {
        self.epoch_source = Some(epoch_source);
        self
    }

    /// Uses `id_source` instead of the operating system's randomness.
    #[must_use]
    pub fn id_source(mut self, id_source: IdSource) -> Self {
        self.id_source = id_source;
        self
    }

    /// Installs this pipeline, replacing whatever was installed.
    ///
    /// Swap-on-install: a call already in flight holds an `Arc` to the previous
    /// pipeline and finishes against it, so the replacement never frees an
    /// exporter mid-export. That is what the C++ `Lease` existed to arrange;
    /// here `Arc` is the whole mechanism.
    #[must_use = "the scope uninstalls this pipeline when it drops, so discarding \
                  it installs and immediately uninstalls, silently disabling telemetry"]
    pub fn install(self) -> PipelineScope {
        // One producer per installed pipeline: its points and logs are counted
        // from one, under an id no other pipeline draws. An id source that
        // panics gives it none, and the pipeline installs anyway — telemetry
        // never fails the caller — with the unset producer the sequence issues
        // nothing under, so every point and log is dropped as
        // `DropReason::Identity`, counted, rather than numbered under an id that
        // no other pipeline is kept from drawing.
        let producer: TraceId =
            crate::context::contained(|| self.id_source.trace_id()).unwrap_or_default();
        let observations = ObservationSequence::new(producer, 1);
        let pipeline = Arc::new(Pipeline {
            exporter: self.exporter,
            sampler: self.sampler,
            epoch_source: self.epoch_source.unwrap_or_else(system_epoch_source),
            id_source: self.id_source,
            observations,
            unsampled: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            misuse: AtomicU64::new(0),
            identity: AtomicU64::new(0),
        });
        // Replace under the lock; flush the displaced pipeline outside it. Two
        // reasons, and both were bugs here: assigning over the slot drops the
        // old `Arc` *in place*, so an exporter whose own `Drop` records anything
        // re-enters this non-reentrant `RwLock` and deadlocks; and whatever the
        // displaced exporter had buffered was previously discarded uncounted.
        let installed = Arc::clone(&pipeline);
        let previous = write_slot().replace(pipeline);
        if let Some(previous) = previous.as_ref() {
            previous.exporter.flush();
        }
        PipelineScope {
            installed,
            previous,
        }
    }
}

/// The process-global slot.
///
/// `RwLock` rather than `Mutex`: reads outnumber writes by the ratio of
/// telemetry calls to installs, and every read is a clone of one `Arc`.
fn slot() -> &'static RwLock<Option<Arc<Pipeline>>> {
    static SLOT: RwLock<Option<Arc<Pipeline>>> = RwLock::new(None);
    &SLOT
}

fn write_slot() -> std::sync::RwLockWriteGuard<'static, Option<Arc<Pipeline>>> {
    slot()
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// What the installed pipeline has dropped, by reason. All zero when none is
/// installed: there is then nothing to count on, and every record is dropped
/// by design.
#[must_use]
pub fn drop_counts() -> DropCounts {
    installed_pipeline().map_or_else(DropCounts::default, |pipeline| pipeline.drop_counts())
}

/// `drop_counts().total()`.
#[must_use]
pub fn dropped_count() -> u64 {
    drop_counts().total()
}

/// The installed pipeline, if any, as a keep-alive snapshot.
///
/// The `Arc` is what the C++ `Lease<const Pipeline>` was for: an export in
/// flight on one thread cannot be freed by a concurrent install or shutdown on
/// another. Holding it is not a lock — nothing blocks behind the caller.
#[must_use]
pub fn installed_pipeline() -> Option<Arc<Pipeline>> {
    slot()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Flushes and uninstalls the pipeline.
///
/// Safe to call with nothing installed, and safe to call twice.
pub fn shutdown_pipeline() {
    // Take the pipeline out under the lock, then flush *outside* it: an
    // exporter's flush can be slow (a network write, an fsync), and holding the
    // global write lock across it would stall every recording call in the
    // process — the opposite of "telemetry never fails the caller".
    let previous = write_slot().take();
    if let Some(pipeline) = previous {
        pipeline.exporter.flush();
    }
}

/// Uninstalls the pipeline when it goes out of scope.
///
/// Returned by [`PipelineBuilder::install`] so the common shape — install at the
/// top of `main`, flush on every exit path including an early `return` — needs
/// no explicit call. **This is the synchronous flush-on-drop mode**, and it is
/// the default: a one-shot process exits after one operation and cannot host a
/// batching task, so batching is opt-in (the `batched` feature) rather than the
/// only path.
///
/// Neither `Clone` nor `Copy`: a scope owns the process-global pipeline for its
/// lifetime, so there is nothing to transfer.
///
/// A scope restores what it displaced rather than clearing the slot, so nesting
/// behaves the way the shape suggests: an inner scope's exit hands the pipeline
/// back to the outer one instead of leaving the process with none.
///
/// It restores only when the slot still holds *its own* pipeline. Anything else
/// means something has already taken over — an explicit [`shutdown_pipeline`],
/// or a scope dropped out of order — and putting the displaced pipeline back
/// then would resurrect one the process deliberately took down. So drop scopes
/// in reverse creation order, which a binding in `main` does for free; a scope
/// dropped out of turn is a no-op rather than a surprise.
#[derive(Debug)]
#[must_use = "a pipeline stays installed only while its scope is alive"]
pub struct PipelineScope {
    // What this scope installed, so its drop can tell whether it is still the
    // one in force.
    installed: Arc<Pipeline>,
    // What was installed before this scope, restored on drop. Also blocks
    // construction outside this module, so the only way to hold one is to have
    // installed a pipeline.
    previous: Option<Arc<Pipeline>>,
}

impl Drop for PipelineScope {
    fn drop(&mut self) {
        let restored = self.previous.take();
        // Swap under the lock, flush outside it — same reasoning as `install`.
        let current = {
            let mut slot = write_slot();
            let ours = slot
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &self.installed));
            if !ours {
                return;
            }
            std::mem::replace(&mut *slot, restored)
        };
        if let Some(current) = current {
            current.exporter.flush();
        }
    }
}
