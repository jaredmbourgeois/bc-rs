//! The exporter seam and the sampler.

use std::sync::Arc;

use crate::log_record::LogRecord;
use crate::metrics::MetricPoint;
use crate::types::{Span, TraceId};

/// Where finished telemetry goes.
///
/// A real trait rather than a closure: it carries four methods, so the family's
/// "a one-method trait is ceremony" rule does not apply. Implementations take
/// `&self` because the pipeline shares one exporter across threads — anything
/// mutable inside owns its own synchronization.
///
/// Every method is **infallible by signature**, and that is the crate's central
/// rule rather than an oversight: telemetry never fails the caller. An exporter
/// that cannot write drops the batch and counts it.
pub trait TelemetryExporter: Send + Sync + std::fmt::Debug {
    /// Exports a batch of finished spans.
    fn export_spans(&self, spans: &[Span]);
    /// Exports a batch of metric points.
    fn export_metrics(&self, metrics: &[MetricPoint]);
    /// Exports a batch of log records.
    fn export_logs(&self, logs: &[LogRecord]);
    /// Flushes anything buffered.
    fn flush(&self);
    /// How many records this exporter has dropped rather than written.
    ///
    /// Dropping is permitted; hiding the drop is not.
    ///
    /// Deliberately **required**, with no default. A defaulted `0` is the wrong
    /// default for the one method whose entire purpose is that losses stay
    /// visible: every new exporter would silently report "nothing lost" until
    /// someone remembered to override it, and the omission looks identical to
    /// an exporter that genuinely cannot drop. An implementor that cannot drop
    /// writes `0` and says so in a comment; that is one line, and it is a
    /// decision rather than an oversight.
    fn dropped(&self) -> u64;
}

/// What a sampler decided about a span.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SamplingDecision {
    /// Do not record the span at all.
    Drop = 0,
    /// Record it locally but do not mark it sampled downstream.
    RecordOnly = 1,
    /// Record it and mark it sampled.
    #[default]
    RecordAndSample = 2,
}

/// The stored decision closure.
///
/// A **private** alias, naming only the boxed type so the struct field below
/// reads. The seam itself stays the nominal `Sampler`, because no DI seam may
/// *be* a bare alias: only a nominal type can later gain `Debug`, a method, and
/// an FFI shape without breaking every call site.
type DecideFn = dyn Fn(&TraceId, &str) -> SamplingDecision + Send + Sync;

/// Decides whether a span is sampled.
///
/// A nominal struct rather than a bare `Arc<dyn Fn…>` alias, because an alias
/// has no `Debug` and can never grow a method or a foreign representation
/// without breaking every call site.
#[derive(Clone)]
pub struct Sampler(Arc<DecideFn>);

impl Sampler {
    /// Wraps any closure as a sampler.
    #[must_use]
    pub fn new(
        decide: impl Fn(&TraceId, &str) -> SamplingDecision + Send + Sync + 'static,
    ) -> Self {
        Self(Arc::new(decide))
    }

    /// Applies the sampler.
    #[must_use]
    pub fn decide(&self, trace_id: &TraceId, span_name: &str) -> SamplingDecision {
        (self.0)(trace_id, span_name)
    }
}

impl std::fmt::Debug for Sampler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Sampler(..)")
    }
}

impl Default for Sampler {
    fn default() -> Self {
        always_on_sampler()
    }
}

/// A [`Sampler`] that samples everything.
#[must_use]
pub fn always_on_sampler() -> Sampler {
    Sampler::new(|_, _| SamplingDecision::RecordAndSample)
}

/// A [`Sampler`] that samples a deterministic `ratio` of traces.
///
/// The decision is a function of the trace id alone, so every span in a trace
/// agrees and a distributed trace is not sampled in fragments.
///
/// The C++ original clamps both endpoints to avoid undefined behaviour: at ratio
/// `1.0` the threshold `ratio * (f64)UINT64_MAX` rounds up to 2^64, and casting
/// that to `u64` is UB in C++ — which on common targets wraps to 0 and drops
/// *every* span while claiming to sample all of them. Rust's `as` saturates
/// instead, so the hazard is gone, but the endpoints stay explicit: they are
/// what a reader checks first, and `>= 1.0` should not depend on a rounding
/// argument to mean "everything".
#[must_use]
pub fn probability_sampler(ratio: f64) -> Sampler {
    Sampler::new(move |trace_id, _| {
        if ratio >= 1.0 {
            return SamplingDecision::RecordAndSample;
        }
        if ratio <= 0.0 || ratio.is_nan() {
            return SamplingDecision::Drop;
        }
        // The first 8 bytes of the trace id, big-endian, as a deterministic hash.
        let mut hash: u64 = 0;
        for byte in &trace_id.bytes[..8] {
            hash = (hash << 8) | u64::from(*byte);
        }
        #[allow(clippy::cast_precision_loss, clippy::cast_sign_loss)]
        #[allow(clippy::cast_possible_truncation)]
        let threshold = (ratio * (u64::MAX as f64)) as u64;
        if hash < threshold {
            SamplingDecision::RecordAndSample
        } else {
            SamplingDecision::Drop
        }
    })
}
