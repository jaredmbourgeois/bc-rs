//! What a run releases while it is still running.
//!
//! Carrier recycling itself is not observable from outside the crate — the
//! reused allocation holds no value between stages — so it is measured by
//! `scripts/bench.sh`, not asserted here. What *is* observable, and what these
//! tests pin, is the arena's lifetime contract: a run holds its producers in a
//! generational arena so a recycled carrier has a stable address to return to,
//! and a spent producer must be released the moment it can no longer be
//! advanced. A run that kept them would grow with the number of outputs it had
//! ever produced rather than with the work still in flight.
//!
//! The third test is the other side of that contract, and is a
//! characterization rather than a requirement: only the *producer* arena
//! releases. Transforms and pipeline nodes live in plain `Vec`s that are
//! appended to and never shrunk, so an expansion carrying its own erased stage
//! retains that stage's boxed closure — and whatever it captured — for the
//! run's lifetime. The README's rejected-approaches list argues why they stay
//! `Vec`s; this pins what that costs, so the argument is answerable with a
//! number rather than an intuition.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use bc_nodular_architecture::{NodeResult, Run, RunStep};

/// A one-item source that reports when it is dropped.
///
/// Held by the producer a `flat_map` expansion creates, so its drop count is
/// exactly the number of spent producers the run has let go of.
struct Reporting {
    value: Option<i32>,
    dropped: Arc<AtomicUsize>,
}

impl Iterator for Reporting {
    type Item = i32;

    fn next(&mut self) -> Option<i32> {
        self.value.take()
    }
}

impl Drop for Reporting {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
    }
}

fn drain(mut run: Run<i32>) -> (Vec<i32>, Run<i32>) {
    let mut outputs = Vec::new();
    loop {
        match run.advance() {
            RunStep::Output(output) => outputs.push(output),
            RunStep::Continue => {}
            RunStep::Complete => return (outputs, run),
        }
    }
}

#[test]
fn a_spent_producer_is_released_before_the_run_that_made_it_ends() {
    const EXPANSIONS: i32 = 256;

    let dropped = Arc::new(AtomicUsize::new(0));
    let counting = Arc::clone(&dropped);

    let run = Run::new(0..EXPANSIONS).flat_map(move |value| {
        NodeResult::run(Reporting {
            value: Some(value),
            dropped: Arc::clone(&counting),
        })
    });

    // The run is deliberately still alive at the assertion: every expansion has
    // completed, so every producer they created is spent and unreachable, and
    // releasing them at drop instead of at completion would be indistinguishable
    // from never releasing them at all for a run that keeps producing.
    let (outputs, run) = drain(run);

    assert_eq!(outputs, (0..EXPANSIONS).collect::<Vec<i32>>());
    assert_eq!(
        dropped.load(Ordering::Relaxed),
        EXPANSIONS as usize,
        "the run is holding spent producers it can no longer advance"
    );
    drop(run);
}

#[test]
fn a_spent_producer_is_released_even_when_stages_still_follow_it() {
    const EXPANSIONS: i32 = 256;

    let dropped = Arc::new(AtomicUsize::new(0));
    let counting = Arc::clone(&dropped);

    // A stage after the expansion means each produced value is still travelling
    // when its producer completes, which is the case the release has to be safe
    // for: the carrier it filled is recycled a step before the producer runs dry.
    let run = Run::new(0..EXPANSIONS)
        .flat_map(move |value| {
            NodeResult::run(Reporting {
                value: Some(value),
                dropped: Arc::clone(&counting),
            })
        })
        .map(|value| value * 2);

    let (outputs, run) = drain(run);

    assert_eq!(
        outputs,
        (0..EXPANSIONS).map(|value| value * 2).collect::<Vec<i32>>()
    );
    assert_eq!(
        dropped.load(Ordering::Relaxed),
        EXPANSIONS as usize,
        "the run is holding spent producers it can no longer advance"
    );
    drop(run);
}

/// State captured by a transform closure, reporting when the closure that owns
/// it is dropped.
///
/// A transform is the interesting case precisely because it is not just an
/// index: it is a boxed closure, and whatever it captured stays alive as long
/// as the closure does.
struct RetainedByTheTransform {
    dropped: Arc<AtomicUsize>,
}

impl RetainedByTheTransform {
    /// Reads the captured state, so the closure genuinely owns this value
    /// rather than merely naming it.
    fn admits(&self, value: i32) -> bool {
        self.dropped.load(Ordering::Relaxed) < usize::MAX && value >= 0
    }
}

impl Drop for RetainedByTheTransform {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
    }
}

/// A characterization test, not a defect report: it records what an expansion
/// carrying its own erased stage costs today, so the cost is measured rather
/// than assumed.
///
/// The producer arena releases; the transform and pipeline tables do not, and
/// the README's rejected-approaches list explains why they stay `Vec`s — a
/// generational handle would widen `Pipeline`, which sits inline in every
/// `Operation` the work deque moves. What that trade actually costs is *this*:
/// the transform closures, and everything they captured, live until the run
/// does.
///
/// Reaching the tables at all takes a specific shape. An expansion returning
/// `None`, `Output`, `Outputs`, `run(iter)`, or a `map` chain under the fusion
/// limit touches neither table, which is why the two tests above — whose
/// expansions are bare `run(iter)` — see no retention to report.
#[test]
fn an_expansions_own_transform_is_retained_until_the_run_ends() {
    const EXPANSIONS: i32 = 256;

    let producers_dropped = Arc::new(AtomicUsize::new(0));
    let transforms_dropped = Arc::new(AtomicUsize::new(0));
    let counting_producers = Arc::clone(&producers_dropped);
    let counting_transforms = Arc::clone(&transforms_dropped);

    let run = Run::new(0..EXPANSIONS).flat_map(move |value| {
        let retained = RetainedByTheTransform {
            dropped: Arc::clone(&counting_transforms),
        };
        // `filter_map` is deliberately not fused, so this inner stage is
        // erased and lands in the outer run's transform table when the
        // expansion splices in.
        NodeResult::Run(
            Run::new(Reporting {
                value: Some(value),
                dropped: Arc::clone(&counting_producers),
            })
            .filter_map(move |output| retained.admits(output).then_some(output)),
        )
    });

    let (outputs, run) = drain(run);
    assert_eq!(outputs, (0..EXPANSIONS).collect::<Vec<i32>>());

    // The two halves of the trade, asserted together so neither can drift
    // silently: the producer half is released during the run…
    assert_eq!(
        producers_dropped.load(Ordering::Relaxed),
        EXPANSIONS as usize,
        "the run is holding spent producers it can no longer advance"
    );
    // …and the transform half is not.
    assert_eq!(
        transforms_dropped.load(Ordering::Relaxed),
        0,
        "the transform table gained a release path; the README's rejected-approaches \
         entry and this test both describe a trade that no longer exists"
    );

    drop(run);
    assert_eq!(
        transforms_dropped.load(Ordering::Relaxed),
        EXPANSIONS as usize,
        "dropping the run must release every transform it accumulated"
    );
}
