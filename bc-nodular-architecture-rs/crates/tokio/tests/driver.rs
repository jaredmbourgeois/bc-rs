//! The tokio driver against the core's stepping API.
//!
//! Every wait here is a rendezvous through the channel or a task join — never a
//! sleep, which would pass on a fast box and flake on a loaded one.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bc_date_time::{Epoch, fixed_epoch_source};
use bc_nodular_architecture::{
    Input, NodeLoop, NodeResult, Nodule, NoduleEventSequence, NoduleLoop, Output, SharedDependency,
    Submission,
};

const CAPACITY: NonZeroUsize = NonZeroUsize::new(8).expect("8 is not zero");
const QUANTUM: NonZeroUsize = NonZeroUsize::new(4).expect("4 is not zero");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CountdownInput {
    Tick,
    Cascade,
}

impl Input for CountdownInput {
    fn sequence(&self) -> NoduleEventSequence {
        0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CountdownOutput {
    remaining: usize,
    feeds_back: bool,
}

impl Output for CountdownOutput {
    fn sequence(&self) -> NoduleEventSequence {
        0
    }
}

/// Emits one output per tick, and on `Cascade` feeds itself back until it
/// reaches zero — the shape that forces the driver to yield mid-cascade.
struct Countdown;

impl Nodule for Countdown {
    type Input = CountdownInput;
    type Output = CountdownOutput;
    type State = usize;
    type Dependency = SharedDependency;

    fn receive(
        input: &Self::Input,
        state: &mut usize,
        _dependency: &SharedDependency,
    ) -> NodeResult<CountdownOutput> {
        match input {
            CountdownInput::Tick => NodeResult::Output(CountdownOutput {
                remaining: *state,
                feeds_back: false,
            }),
            CountdownInput::Cascade => {
                *state -= 1;
                NodeResult::Output(CountdownOutput {
                    remaining: *state,
                    feeds_back: true,
                })
            }
        }
    }
}

impl NoduleLoop for Countdown {
    fn input_from_output(
        output: &CountdownOutput,
        _state: &usize,
        _dependency: &SharedDependency,
    ) -> Option<CountdownInput> {
        (output.feeds_back && output.remaining > 0).then_some(CountdownInput::Cascade)
    }
}

fn dependency() -> SharedDependency {
    SharedDependency::new(fixed_epoch_source(Epoch::ZERO))
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("a current-thread runtime")
}

#[test]
fn the_driver_returns_the_final_state_once_every_sender_is_dropped() {
    let observed = Arc::new(AtomicUsize::new(0));
    let recording = Arc::clone(&observed);

    let state = runtime().block_on(async move {
        let (submitter, submissions) = bc_nodular_tokio::channel::<Countdown>(CAPACITY);
        for _ in 0..3 {
            submitter
                .send(Submission::Input(CountdownInput::Tick))
                .await
                .expect("capacity is free");
        }
        drop(submitter);

        let node_loop = NodeLoop::<Countdown>::with_output(dependency(), 7, move |_| {
            recording.fetch_add(1, Ordering::Relaxed);
        });
        bc_nodular_tokio::run(node_loop, submissions, QUANTUM).await
    });

    assert_eq!(state, 7);
    assert_eq!(observed.load(Ordering::Relaxed), 3);
}

#[test]
fn work_already_accepted_drains_before_the_driver_returns() {
    let delivered = Arc::new(AtomicUsize::new(0));
    let recording = Arc::clone(&delivered);

    // Every submission is queued and the sender dropped *before* the driver ever
    // runs, so returning early would visibly lose accepted work.
    let state = runtime().block_on(async move {
        let (submitter, submissions) = bc_nodular_tokio::channel::<Countdown>(CAPACITY);
        submitter
            .send(Submission::Input(CountdownInput::Cascade))
            .await
            .expect("capacity is free");
        drop(submitter);

        let node_loop = NodeLoop::<Countdown>::with_output(dependency(), 5, move |_| {
            recording.fetch_add(1, Ordering::Relaxed);
        });
        bc_nodular_tokio::run(node_loop, submissions, QUANTUM).await
    });

    assert_eq!(state, 0, "the cascade ran to the bottom");
    assert_eq!(delivered.load(Ordering::Relaxed), 5);
}

#[test]
fn a_competing_task_progresses_while_a_cascade_is_still_running() {
    let ticks = Arc::new(AtomicUsize::new(0));
    let counting = Arc::clone(&ticks);

    let observed_during_cascade = runtime().block_on(async move {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async move {
                let (submitter, submissions) = bc_nodular_tokio::channel::<Countdown>(CAPACITY);
                submitter
                    .send(Submission::Input(CountdownInput::Cascade))
                    .await
                    .expect("capacity is free");
                drop(submitter);

                // Counts how many times it is polled while the driver works. A
                // driver that ran the whole cascade without yielding would leave
                // this at zero.
                let competitor = tokio::task::spawn_local(async move {
                    for _ in 0..64 {
                        counting.fetch_add(1, Ordering::Relaxed);
                        tokio::task::yield_now().await;
                    }
                });

                let node_loop = NodeLoop::<Countdown>::new(dependency(), 200);
                let state = bc_nodular_tokio::run(node_loop, submissions, QUANTUM).await;
                competitor.abort();

                (state, ticks.load(Ordering::Relaxed))
            })
            .await
    });

    let (state, competitor_ticks) = observed_during_cascade;
    assert_eq!(state, 0, "the cascade ran to the bottom");
    assert!(
        competitor_ticks > 1,
        "the driver never yielded: competitor ran {competitor_ticks} times"
    );
}

#[test]
fn the_driver_returns_at_a_close_submission_while_a_sender_is_still_alive() {
    // The submitter is deliberately held across the whole call. A driver that
    // returned only when its senders dropped would never return here, so the
    // timeout is the failure bound rather than a wait: reaching it is the bug.
    let state = runtime().block_on(async move {
        let (submitter, submissions) = bc_nodular_tokio::channel::<Countdown>(CAPACITY);
        submitter
            .send(Submission::Input(CountdownInput::Cascade))
            .await
            .expect("capacity is free");
        submitter
            .send(Submission::Close)
            .await
            .expect("capacity is free");

        let node_loop = NodeLoop::<Countdown>::new(dependency(), 3);
        let state = tokio::time::timeout(
            Duration::from_secs(10),
            bc_nodular_tokio::run(node_loop, submissions, QUANTUM),
        )
        .await
        .expect("the driver returns once acceptance closes, not once senders drop");
        drop(submitter);
        state
    });

    assert_eq!(state, 0, "work accepted before the close marker still ran");
}

#[test]
fn a_submission_arriving_mid_cascade_is_taken_up_before_the_cascade_ends() {
    const DEPTH: usize = 200;
    const TICKS: usize = 8;

    // The cascade's own outputs are the clock. Each time the feeder gets a
    // submission into the one-slot channel it records how many outputs the loop
    // has produced by then, so the assertion is about *when* work was taken up,
    // not merely that it eventually was. A driver that only looked at the channel
    // once its queue ran dry would record `DEPTH` for every send after the first:
    // the feeder would sit on a full channel for the whole cascade.
    let produced = Arc::new(AtomicUsize::new(0));
    let counting = Arc::clone(&produced);
    let accepted_at = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recording = Arc::clone(&accepted_at);

    runtime().block_on(async move {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async move {
                let (submitter, submissions) = bc_nodular_tokio::channel::<Countdown>(
                    NonZeroUsize::new(1).expect("1 is not zero"),
                );

                let feeding = tokio::task::spawn_local(async move {
                    submitter
                        .send(Submission::Input(CountdownInput::Cascade))
                        .await
                        .expect("the driver is running");
                    for _ in 0..TICKS {
                        submitter
                            .send(Submission::Input(CountdownInput::Tick))
                            .await
                            .expect("the driver keeps taking submissions");
                        recording
                            .lock()
                            .expect("no task panics holding this")
                            .push(counting.load(Ordering::Relaxed));
                    }
                });

                let node_loop =
                    NodeLoop::<Countdown>::with_output(dependency(), DEPTH, move |_| {
                        produced.fetch_add(1, Ordering::Relaxed);
                    });
                bc_nodular_tokio::run(node_loop, submissions, QUANTUM).await;
                feeding.await.expect("the feeder is never cancelled");
            })
            .await;
    });

    let accepted_at = accepted_at
        .lock()
        .expect("the runtime has finished")
        .clone();
    assert_eq!(accepted_at.len(), TICKS, "not every tick was accepted");
    let last = *accepted_at.last().expect("TICKS is not zero");
    assert!(
        last < DEPTH,
        "the last tick was only taken up after {last} outputs, with a cascade of {DEPTH}: \
         the driver holds its channel shut while it has work"
    );
}

#[test]
fn a_submission_arriving_after_a_close_is_never_taken_up() {
    // A pin rather than a red test: this driver already stops reading the
    // channel at a close. It exists so the core's new acceptance state cannot
    // quietly reintroduce the post-close work the other two drivers ran.
    let observed = Arc::new(AtomicUsize::new(0));
    let recording = Arc::clone(&observed);

    let state = runtime().block_on(async move {
        let (submitter, submissions) = bc_nodular_tokio::channel::<Countdown>(CAPACITY);
        submitter
            .send(Submission::Close)
            .await
            .expect("capacity is free");
        submitter
            .send(Submission::Input(CountdownInput::Tick))
            .await
            .expect("capacity is free");

        let node_loop = NodeLoop::<Countdown>::with_output(dependency(), 3, move |_| {
            recording.fetch_add(1, Ordering::Relaxed);
        });
        let state = tokio::time::timeout(
            Duration::from_secs(10),
            bc_nodular_tokio::run(node_loop, submissions, QUANTUM),
        )
        .await
        .expect("the driver returns once acceptance closes");
        drop(submitter);
        state
    });

    assert_eq!(
        observed.load(Ordering::Relaxed),
        0,
        "an input submitted after the close marker still ran"
    );
    assert_eq!(state, 3);
}

#[test]
fn a_competing_task_progresses_while_a_closed_loop_drains() {
    let ticks = Arc::new(AtomicUsize::new(0));
    let counting = Arc::clone(&ticks);

    let observed_during_drain = runtime().block_on(async move {
        let local = tokio::task::LocalSet::new();
        local
            .run_until(async move {
                let (submitter, submissions) = bc_nodular_tokio::channel::<Countdown>(CAPACITY);
                // Both are in the channel before the first quantum, so the close
                // is reached with the whole cascade still queued: every output
                // below is produced on the drain path, not the working one.
                submitter
                    .send(Submission::Input(CountdownInput::Cascade))
                    .await
                    .expect("capacity is free");
                submitter
                    .send(Submission::Close)
                    .await
                    .expect("capacity is free");

                let competitor = tokio::task::spawn_local(async move {
                    for _ in 0..64 {
                        counting.fetch_add(1, Ordering::Relaxed);
                        tokio::task::yield_now().await;
                    }
                });

                let node_loop = NodeLoop::<Countdown>::new(dependency(), 200);
                let state = bc_nodular_tokio::run(node_loop, submissions, QUANTUM).await;
                competitor.abort();
                drop(submitter);

                (state, ticks.load(Ordering::Relaxed))
            })
            .await
    });

    let (state, competitor_ticks) = observed_during_drain;
    assert_eq!(state, 0, "the cascade drained to the bottom");
    assert!(
        competitor_ticks > 1,
        "the drain never yielded: competitor ran {competitor_ticks} times"
    );
}
