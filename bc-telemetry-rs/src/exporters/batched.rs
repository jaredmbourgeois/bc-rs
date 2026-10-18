//! Batched asynchronous export on tokio.
//!
//! **New surface, and deliberately optional.** The C++ exports one record
//! synchronously per call; batching is an improvement this port adds. It is a
//! *feature* rather than the default because a one-shot process — a
//! command-line tool's single invocation — exits after one operation and cannot
//! host a batching task. Synchronous flush-on-drop stays the default path; this
//! wraps any exporter for a process that already owns a runtime, such as a
//! long-running server.
//!
//! # What it drops, and why that is the design
//!
//! The queue is bounded. A full queue drops the newest record and counts it,
//! rather than applying back-pressure to the caller: back-pressure would make
//! instrumentation slow down the work it measures, which is the one thing
//! telemetry must never do. Dropping is permitted; hiding a drop is not, so the
//! count is reported through [`TelemetryExporter::dropped`] like every other.
//! The same count holds a batch whose export panicked in the drain, and every
//! record handed to an exporter built outside a runtime, which has no writer to
//! queue for and never writes on the caller's thread instead.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::mpsc::{Sender, error::TrySendError};
use tokio::task::JoinHandle;

use crate::exporter::TelemetryExporter;
use crate::log_record::LogRecord;
use crate::metrics::MetricPoint;
use crate::types::Span;

/// One batch of work handed to the draining task.
#[derive(Debug)]
enum Batch {
    Spans(Vec<Span>),
    Metrics(Vec<MetricPoint>),
    Logs(Vec<LogRecord>),
    Flush,
    /// Ends the drain loop once everything queued ahead of it has run.
    ///
    /// A sentinel rather than closing the channel: the sender lives in `&self`
    /// and `shutdown` takes `&self`, so it cannot drop the sender — and awaiting
    /// the task while still holding a live sender waits forever.
    Shutdown,
}

impl Batch {
    /// How many records the batch carries: what dropping it loses.
    fn records(&self) -> u64 {
        let count = match self {
            Self::Spans(spans) => spans.len(),
            Self::Metrics(metrics) => metrics.len(),
            Self::Logs(logs) => logs.len(),
            Self::Flush | Self::Shutdown => 0,
        };
        u64::try_from(count).unwrap_or(u64::MAX)
    }
}

/// Queues exports and drains them on a tokio task.
#[derive(Debug)]
pub struct BatchedExporter {
    /// `None` when constructed outside a runtime, in which case every record
    /// is dropped and counted. See [`BatchedExporter::new`].
    sender: Option<Sender<Batch>>,
    /// Kept so [`BatchedExporter::shutdown`] can await the drain. A `Mutex`
    /// because taking it needs `&self` — `TelemetryExporter` is a `&self` trait
    /// and the pipeline holds this behind an `Arc`.
    task: Mutex<Option<JoinHandle<()>>>,
    /// Retained rather than moved into the task, so `dropped()` can include what
    /// the wrapped exporter itself dropped. Moving it in made those drops
    /// permanently unreachable: wrapping an OTLP exporter hid every failed POST.
    inner: Arc<dyn TelemetryExporter>,
    /// What was refused or lost: a full queue's batches, every batch when
    /// there is no runtime, and a batch whose export panicked in the drain —
    /// shared with the draining task for that last, the one loss it sees.
    dropped: Arc<AtomicU64>,
}

impl BatchedExporter {
    /// Wraps `inner`, draining into it from a task on the current runtime.
    ///
    /// `capacity` is the queue depth; a full queue drops rather than blocks.
    ///
    /// Outside a tokio runtime there is no task to drain into `inner`, so every
    /// record is dropped and counted rather than exported inline on the
    /// caller's thread: a writer never makes a request wait on the database,
    /// and an inline export is that wait.
    /// Nor does it panic, as `tokio::spawn` would with "there is no reactor
    /// running": a constructor in a crate whose first rule is that telemetry
    /// never fails the caller must not be the one place that aborts the
    /// program. A mis-wired `main` loses its telemetry, and the drop count says
    /// so; a one-shot process wants the synchronous pipeline, not this.
    ///
    /// **Queued records are not guaranteed to reach `inner` unless
    /// [`shutdown`](Self::shutdown) is awaited.** Dropping the exporter closes
    /// the channel, but if the runtime shuts down first the task is cancelled
    /// with work still queued, and nothing flushes `inner`.
    /// `PipelineScope::drop` cannot await, so a batched pipeline needs an
    /// explicit shutdown before the runtime ends.
    #[must_use]
    pub fn new(inner: Arc<dyn TelemetryExporter>, capacity: usize) -> Self {
        let dropped = Arc::new(AtomicU64::new(0));
        if tokio::runtime::Handle::try_current().is_err() {
            return Self {
                sender: None,
                task: Mutex::new(None),
                inner,
                dropped,
            };
        }
        let drained = Arc::clone(&inner);
        let drain_drops = Arc::clone(&dropped);
        let (sender, mut receiver) = tokio::sync::mpsc::channel(capacity);

        // The inner exporter's methods are synchronous and may block on I/O, so
        // the drain runs them on the blocking pool rather than on a runtime
        // worker. Nothing here holds a lock across an await.
        let task = tokio::spawn(async move {
            while let Some(batch) = receiver.recv().await {
                // The sentinel is handled like any other batch first — it maps
                // to a flush — and only then ends the loop. Breaking before the
                // match is what made `shutdown` hand every queued record to
                // `inner` and then leave `inner` unflushed, with the flush arm
                // below sitting there unreachable, reading as if it ran.
                let last = matches!(batch, Batch::Shutdown);
                let records = batch.records();
                let inner = Arc::clone(&drained);
                let handle = tokio::task::spawn_blocking(move || match batch {
                    Batch::Spans(spans) => inner.export_spans(&spans),
                    Batch::Metrics(metrics) => inner.export_metrics(&metrics),
                    Batch::Logs(logs) => inner.export_logs(&logs),
                    Batch::Flush | Batch::Shutdown => inner.flush(),
                });
                // A panic in an exporter is contained here — telemetry never
                // fails the caller, and the caller is long gone anyway — and
                // its batch is counted, never lost unseen.
                if handle.await.is_err() {
                    drain_drops.fetch_add(records, Ordering::Relaxed);
                }
                if last {
                    break;
                }
            }
        });

        Self {
            sender: Some(sender),
            task: Mutex::new(Some(task)),
            inner,
            dropped,
        }
    }

    /// Drains the queue into `inner`, flushes it, and waits for both.
    ///
    /// This is the only way to guarantee delivery at exit. `flush` merely queues
    /// a request behind the existing backlog and returns; if the runtime stops
    /// before the task drains, those records are gone. Awaiting here is what a
    /// `serve` process should do on its shutdown path.
    ///
    /// The flush is the point, not a courtesy: handing a buffering exporter its
    /// last batch and then stopping would leave that batch in the buffer, which
    /// is the same lost data by a later name.
    pub async fn shutdown(&self) {
        // Queue the sentinel with the blocking `send`, not `try_send`: at
        // shutdown a full queue must be waited on, not dropped. Everything
        // already queued runs first — the channel is FIFO.
        let Some(sender) = self.sender.as_ref() else {
            // Constructed outside a runtime: there is no queue to drain, but
            // `inner` may still be buffering, and delivery is draining *and*
            // flushing. Returning here without the flush left the fallback path
            // with the defect the queued path was corrected for.
            self.inner.flush();
            return;
        };
        let _ = sender.send(Batch::Shutdown).await;
        let task = {
            let mut slot = self
                .task
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            slot.take()
        };
        if let Some(task) = task {
            let _ = task.await;
        }
    }

    /// How many records the queue has refused.
    #[must_use]
    pub fn dropped_count(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    fn enqueue(&self, batch: Batch) {
        let records = batch.records();
        let Some(sender) = self.sender.as_ref() else {
            // Constructed outside a runtime: no writer to queue for, so the
            // batch is dropped and counted rather than written on this thread.
            self.dropped.fetch_add(records, Ordering::Relaxed);
            return;
        };
        match sender.try_send(batch) {
            Ok(()) => {}
            Err(TrySendError::Full(_) | TrySendError::Closed(_)) => {
                self.dropped.fetch_add(records, Ordering::Relaxed);
            }
        }
    }
}

impl TelemetryExporter for BatchedExporter {
    fn export_spans(&self, spans: &[Span]) {
        self.enqueue(Batch::Spans(spans.to_vec()));
    }

    fn export_metrics(&self, metrics: &[MetricPoint]) {
        self.enqueue(Batch::Metrics(metrics.to_vec()));
    }

    fn export_logs(&self, logs: &[LogRecord]) {
        self.enqueue(Batch::Logs(logs.to_vec()));
    }

    /// Queues a flush for the inner exporter.
    ///
    /// Asynchronous by nature: this returns once the request is queued, not once
    /// the inner exporter has finished. A process that needs the stronger
    /// guarantee at exit wants the synchronous path, which is why that one is
    /// the default.
    fn flush(&self) {
        self.enqueue(Batch::Flush);
    }

    /// Queue refusals **plus** whatever the wrapped exporter dropped.
    ///
    /// The inner count used to be unreachable, because `inner` was moved into
    /// the draining task and no handle was kept: wrapping an exporter that
    /// counts its own failures made every one of them invisible.
    fn dropped(&self) -> u64 {
        self.dropped_count().saturating_add(self.inner.dropped())
    }
}
