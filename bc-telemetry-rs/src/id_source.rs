//! Where trace and span identifiers come from.
//!
//! The C++ original calls the OS CSPRNG directly from `random.cpp`
//! (`arc4random_buf` / `getrandom` / `BCryptGenRandom`). Rust cannot reach those
//! without `unsafe`, which this crate forbids, so the call goes through
//! `getrandom` — the same syscalls, per platform.
//!
//! What the port adds is the seam. The C++ has no way to make identifiers
//! deterministic, so nothing that produces one can be asserted exactly; here the
//! generator is an injected [`IdSource`], and a test installs a counting one.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::types::{ObservationId, SpanId, TraceId};

/// The stored fill closure — a private alias, for the reason given on
/// `Sampler`'s: the seam is the nominal `IdSource`, not this name.
type FillFn = dyn Fn(&mut [u8]) + Send + Sync;

/// A source of random bytes for identifiers.
///
/// A nominal struct rather than a bare `Arc<dyn Fn…>` alias, as
/// [`bc_date_time::EpochSource`] already is, because an alias has no `Debug`,
/// cannot satisfy an `impl Fn` bound, and can never grow a method or a foreign
/// representation without breaking every call site.
#[derive(Clone)]
pub struct IdSource(Arc<FillFn>);

impl IdSource {
    /// Wraps any closure that fills a buffer with bytes.
    #[must_use]
    pub fn new(fill: impl Fn(&mut [u8]) + Send + Sync + 'static) -> Self {
        Self(Arc::new(fill))
    }

    /// Mints a trace identifier.
    #[must_use]
    pub fn trace_id(&self) -> TraceId {
        let mut bytes = [0_u8; 16];
        (self.0)(&mut bytes);
        TraceId { bytes }
    }

    /// Mints a span identifier.
    #[must_use]
    pub fn span_id(&self) -> SpanId {
        let mut bytes = [0_u8; 8];
        (self.0)(&mut bytes);
        SpanId { bytes }
    }
}

impl std::fmt::Debug for IdSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // A closure has nothing printable, and drawing bytes to render a `Debug`
        // line would make formatting consume entropy.
        formatter.write_str("IdSource(..)")
    }
}

impl Default for IdSource {
    fn default() -> Self {
        system_id_source()
    }
}

/// An [`IdSource`] backed by the operating system's randomness.
///
/// # Failure
///
/// `getrandom` can fail only where the OS has no entropy source at all. This
/// crate's rule is that telemetry never fails the caller, so a failure yields an
/// **all-zero** identifier rather than a panic — and all-zero is precisely what
/// [`TraceId::is_valid`] reports as invalid, so the condition is visible in the
/// data instead of being hidden or fatal.
#[must_use]
pub fn system_id_source() -> IdSource {
    IdSource::new(|buffer| {
        if getrandom::fill(buffer).is_err() {
            buffer.fill(0);
        }
    })
}

/// An [`IdSource`] that fills every buffer with `byte` — the test source.
#[must_use]
pub fn fixed_id_source(byte: u8) -> IdSource {
    IdSource::new(move |buffer| buffer.fill(byte))
}

/// Issues the [`ObservationId`]s of one producer.
///
/// Each nonzero sequence is issued at most once, from many threads. Zero is the
/// spent mark: once `u64::MAX` has been issued the counter reads zero and
/// issues nothing more, rather than wrap and issue `1` a second time.
///
/// The C++ also refuses to issue in a fork child, whose copy of the counter
/// would repeat its parent's. Rust's standard library cannot fork, and this
/// crate forbids the `unsafe` it would take, so that case has nowhere to arise
/// from inside it.
#[derive(Debug)]
pub(crate) struct ObservationSequence {
    producer: TraceId,
    next: AtomicU64,
}

impl ObservationSequence {
    /// A sequence for `producer` whose first id is `first_sequence`.
    pub(crate) fn new(producer: TraceId, first_sequence: u64) -> Self {
        Self {
            producer,
            next: AtomicU64::new(first_sequence),
        }
    }

    /// The next id, or `None` when the producer is unset — an id source that
    /// could only give zeros — or every sequence has been issued.
    pub(crate) fn next(&self) -> Option<ObservationId> {
        if !self.producer.is_valid() {
            return None;
        }
        // Take the current value and advance past it in one atomic step; nothing is
        // issued once the counter reads zero. This is the compare-and-swap loop
        // `fetch_update` runs, written out because 1.99 deprecates that name for
        // `try_update`, which the 1.85 MSRV does not have.
        let mut current = self.next.load(Ordering::Relaxed);
        let issued = loop {
            if current == 0 {
                return None;
            }
            match self.next.compare_exchange_weak(
                current,
                current.wrapping_add(1),
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(previous) => break previous,
                // Another thread moved the counter first: try again from what it holds now.
                Err(actual) => current = actual,
            }
        };
        Some(ObservationId {
            producer: self.producer,
            sequence: issued,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ObservationSequence;
    use crate::types::TraceId;

    fn producer() -> TraceId {
        TraceId { bytes: [7; 16] }
    }

    #[test]
    fn a_sequence_issues_each_number_once_from_its_first() {
        let sequence = ObservationSequence::new(producer(), 1);
        let issued: Vec<u64> = (0..3)
            .map(|_| sequence.next().expect("an id").sequence)
            .collect();
        assert_eq!(issued, [1, 2, 3]);
    }

    #[test]
    fn the_last_number_is_issued_once_and_then_nothing() {
        let sequence = ObservationSequence::new(producer(), u64::MAX - 1);
        assert_eq!(sequence.next().map(|id| id.sequence), Some(u64::MAX - 1));
        assert_eq!(sequence.next().map(|id| id.sequence), Some(u64::MAX));
        assert_eq!(sequence.next(), None);
        assert_eq!(sequence.next(), None);
    }

    #[test]
    fn an_id_source_renders_without_drawing_bytes() {
        assert_eq!(format!("{:?}", super::fixed_id_source(1)), "IdSource(..)");
    }

    #[test]
    fn an_unset_producer_issues_nothing() {
        let sequence = ObservationSequence::new(TraceId::default(), 1);
        assert_eq!(sequence.next(), None);
    }

    #[test]
    fn many_threads_never_share_a_number() {
        let sequence = ObservationSequence::new(producer(), 1);
        let mut issued: Vec<u64> = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        (0..1_000)
                            .map(|_| sequence.next().expect("an id").sequence)
                            .collect::<Vec<u64>>()
                    })
                })
                .collect();
            workers
                .into_iter()
                .flat_map(|worker| worker.join().expect("a worker finished"))
                .collect()
        });
        issued.sort_unstable();
        issued.dedup();
        assert_eq!(issued.len(), 8_000);
        assert_eq!(issued.first(), Some(&1));
        assert_eq!(issued.last(), Some(&8_000));
    }
}
