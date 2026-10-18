//! The thread driver and the stepping API, covering the semantics the removed
//! Tokio adapter used to pin: bounded backpressure, acceptance versus
//! processing, ordered close, draining already accepted work, rejected-payload
//! recovery, cooperative progress, and panic propagation.

mod support;

use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};

use bc_nodular_architecture::{
    Acceptance, Input, NodeLoop, NodeResult, Nodule, NoduleEventSequence, NoduleLoop, Output,
    Progress, SharedDependency, Submission, ThreadNodeLoopRunner,
};
use support::{
    Countdown, CountdownInput, CountdownOutput, CountdownState, OneShot, OneShotInput,
    OneShotOutput, dependency,
};

fn recording_observer() -> (
    Arc<Mutex<Vec<usize>>>,
    impl FnMut(&support::CountdownOutput) + Send + 'static,
) {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&observed);
    (observed, move |output: &support::CountdownOutput| {
        recorded
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(output.remaining);
    })
}

fn recorded(observed: &Arc<Mutex<Vec<usize>>>) -> Vec<usize> {
    observed
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

#[test]
fn countdown_drains_accepted_work_before_closing() {
    let (observed, record) = recording_observer();
    let (handle, runner) = ThreadNodeLoopRunner::<Countdown>::with_output(
        dependency(),
        CountdownState { remaining: 5 },
        NonZeroUsize::MIN,
        record,
    );

    handle.receive(CountdownInput::Tick).unwrap();
    drop(handle);
    let state = runner.run();

    assert_eq!(recorded(&observed), [4, 3, 2, 1, 0]);
    assert_eq!(state, CountdownState { remaining: 0 });
}

#[test]
fn run_delivers_multiple_outputs_depth_first() {
    let (observed, record) = recording_observer();
    let (handle, runner) = ThreadNodeLoopRunner::<Countdown>::with_output(
        dependency(),
        CountdownState { remaining: 2 },
        NonZeroUsize::MIN,
        record,
    );

    handle.receive(CountdownInput::Run).unwrap();
    drop(handle);
    let state = runner.run();

    assert_eq!(recorded(&observed), [2, 1, 0, 9]);
    assert_eq!(state, CountdownState { remaining: 0 });
}

#[test]
fn a_handle_reports_a_closed_runner_and_returns_the_payload() {
    let (handle, runner) = ThreadNodeLoopRunner::<Countdown>::new(
        dependency(),
        CountdownState { remaining: 1 },
        NonZeroUsize::MIN,
    );

    handle.close().unwrap();
    let state = runner.run();

    let rejected = handle
        .receive(CountdownInput::Tick)
        .expect_err("a closed runner rejects further inputs");
    assert_eq!(rejected.into_inner(), CountdownInput::Tick);
    assert_eq!(state, CountdownState { remaining: 1 });
}

#[test]
fn the_runner_moves_to_another_thread_and_closing_ends_it() {
    let (observed, record) = recording_observer();
    let (handle, runner) = ThreadNodeLoopRunner::<Countdown>::with_output(
        dependency(),
        CountdownState { remaining: 3 },
        NonZeroUsize::new(4).unwrap(),
        record,
    );

    let worker = std::thread::spawn(move || runner.run());
    handle.receive(CountdownInput::Tick).unwrap();
    handle.close().unwrap();
    let state = worker.join().expect("the runner thread finished cleanly");

    assert_eq!(recorded(&observed), [2, 1, 0]);
    assert_eq!(state, CountdownState { remaining: 0 });
}

#[test]
fn a_close_marker_drains_work_already_accepted_behind_it() {
    let (observed, record) = recording_observer();
    let (handle, runner) = ThreadNodeLoopRunner::<Countdown>::with_output(
        dependency(),
        CountdownState { remaining: 2 },
        NonZeroUsize::new(8).unwrap(),
        record,
    );

    // All three are accepted before the runner starts, so the close marker sits
    // in the queue ahead of the second input.
    handle.receive(CountdownInput::Tick).unwrap();
    handle.close().unwrap();
    let state = runner.run();

    // The first input's whole causal chain runs; nothing after the marker does.
    assert_eq!(recorded(&observed), [1, 0]);
    assert_eq!(state, CountdownState { remaining: 0 });
}

#[test]
fn a_submission_blocks_exactly_while_the_queue_is_full() {
    let delivered = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&delivered);
    let (handle, runner) = ThreadNodeLoopRunner::<OneShot>::with_output(
        dependency(),
        0,
        NonZeroUsize::MIN,
        move |output: &OneShotOutput| {
            recorded
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(output.0);
        },
    );

    // Capacity is one and nothing is draining yet, so the second send cannot be
    // accepted until the runner takes the first. The barrier proves the sender
    // really parks rather than the test racing ahead of it.
    handle.receive(OneShotInput(1)).unwrap();

    let accepted = Arc::new(AtomicUsize::new(0));
    let sender_accepted = Arc::clone(&accepted);
    let parked = Arc::new(Barrier::new(2));
    let sender_parked = Arc::clone(&parked);
    let sender = std::thread::spawn(move || {
        sender_parked.wait();
        handle.receive(OneShotInput(2)).unwrap();
        sender_accepted.store(1, Ordering::SeqCst);
        drop(handle);
    });

    parked.wait();
    assert_eq!(accepted.load(Ordering::SeqCst), 0, "the sender is parked");

    let received = runner.run();
    sender.join().expect("the sending thread finished cleanly");

    assert_eq!(accepted.load(Ordering::SeqCst), 1);
    assert_eq!(received, 2, "both inputs were processed, not just accepted");
    assert_eq!(
        delivered
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone(),
        [2, 4]
    );
}

#[test]
fn close_and_drain_finishes_with_a_retained_handle_and_accepted_work() {
    let (observed, record) = recording_observer();
    let (handle, runner) = ThreadNodeLoopRunner::<Countdown>::with_output(
        dependency(),
        CountdownState { remaining: 2 },
        NonZeroUsize::new(4).unwrap(),
        record,
    );

    handle.receive(CountdownInput::Tick).unwrap();
    let state = runner.close_and_drain();

    // The handle outlives the runner, so this proves the drain does not wait on
    // every sender dropping the way `run` does.
    assert!(handle.receive(CountdownInput::Tick).is_err());
    assert_eq!(recorded(&observed), [1, 0]);
    assert_eq!(state, CountdownState { remaining: 0 });
}

#[test]
fn a_run_panic_reaches_the_joining_thread_after_its_delivered_output() {
    let (observed, record) = recording_observer();
    let (handle, runner) = ThreadNodeLoopRunner::<Countdown>::with_output(
        dependency(),
        CountdownState { remaining: 1 },
        NonZeroUsize::new(4).unwrap(),
        record,
    );

    let worker = std::thread::spawn(move || runner.run());
    handle.receive(CountdownInput::PanicRun).unwrap();
    drop(handle);
    let panic = worker.join().expect_err("the run panic reaches the joiner");

    let message = panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
        .expect("the panic carries a message");
    assert_eq!(message, "panic after a delivered output");
    assert_eq!(recorded(&observed), [7]);
}

// --- the stepping API ---

#[test]
fn stepping_reports_idle_only_once_the_queue_is_empty() {
    let mut node_loop = NodeLoop::<Countdown>::new(dependency(), CountdownState { remaining: 2 });

    assert_eq!(node_loop.step(), Progress::Idle);
    let _ = node_loop.submit(Submission::Input(CountdownInput::Tick));

    let mut steps = 0;
    while node_loop.step() == Progress::Worked {
        steps += 1;
    }

    assert!(steps > 2, "the cascade took more than one step: {steps}");
    assert_eq!(node_loop.state(), &CountdownState { remaining: 0 });
}

#[test]
fn a_quantum_bounds_the_work_one_step_batch_performs() {
    const DEPTH: usize = 100_000;

    let mut node_loop =
        NodeLoop::<Countdown>::new(dependency(), CountdownState { remaining: DEPTH });
    let _ = node_loop.submit(Submission::Input(CountdownInput::Tick));

    // Each quantum returns control, which is what lets a caller's executor
    // interleave its own work without breaking the causal chain.
    let quantum = NonZeroUsize::new(64).unwrap();
    let mut quanta = 0;
    while node_loop.step_quantum(quantum) == Progress::Worked {
        quanta += 1;
    }

    assert!(quanta > DEPTH / 64, "each quantum was bounded: {quanta}");
    assert_eq!(node_loop.state(), &CountdownState { remaining: 0 });
}

#[test]
fn a_quantum_yields_between_deep_transform_stages() {
    const STAGES: usize = 1_000;

    let mut node_loop = NodeLoop::<DeepMap>::new(dependency(), ());
    let _ = node_loop.submit(Submission::Input(DeepMapInput));

    // A single output crossing a thousand transforms still hands control back
    // between quanta rather than running the chain to completion in one step.
    let quantum = NonZeroUsize::new(64).unwrap();
    let mut quanta = 0;
    while node_loop.step_quantum(quantum) == Progress::Worked {
        quanta += 1;
    }

    assert!(quanta > STAGES / 64, "the deep chain was bounded: {quanta}");
}

#[test]
fn a_close_submission_reports_that_acceptance_ended() {
    let mut node_loop = NodeLoop::<Countdown>::new(dependency(), CountdownState { remaining: 1 });

    assert_eq!(
        node_loop.submit(Submission::Input(CountdownInput::Tick)),
        Acceptance::Open
    );
    assert_eq!(node_loop.submit(Submission::Close), Acceptance::Closed);
}

#[test]
fn a_stepping_loop_discards_and_reports_every_submission_after_a_close() {
    let (observed, record) = recording_observer();
    let mut node_loop =
        NodeLoop::<Countdown>::with_output(dependency(), CountdownState { remaining: 5 }, record);

    assert_eq!(node_loop.submit(Submission::Close), Acceptance::Closed);
    assert_eq!(
        node_loop.submit(Submission::Input(CountdownInput::Tick)),
        Acceptance::Closed,
        "a loop past its close marker reported itself open"
    );
    assert_eq!(
        node_loop.submit(Submission::Output(CountdownOutput {
            remaining: 9,
            feeds_back: false,
        })),
        Acceptance::Closed,
        "a loop past its close marker reported itself open"
    );

    while node_loop.step() == Progress::Worked {}
    let state = node_loop.into_state();

    // The countdown is deliberately left with room to run: had either
    // submission been accepted it would have counted down and been observed,
    // rather than underflowing into a panic that proves nothing.
    assert_eq!(
        recorded(&observed),
        [],
        "work submitted after the close marker still ran"
    );
    assert_eq!(state, CountdownState { remaining: 5 });
}

#[test]
fn the_thread_runner_discards_work_submitted_after_a_close() {
    let (observed, record) = recording_observer();
    let (handle, runner) = ThreadNodeLoopRunner::<Countdown>::with_output(
        dependency(),
        CountdownState { remaining: 5 },
        NonZeroUsize::new(4).unwrap(),
        record,
    );

    // Both sit in the channel before the runner starts, so the order is the
    // queue's rather than a race: the marker is reached before the input.
    handle.close().unwrap();
    handle.receive(CountdownInput::Tick).unwrap();
    drop(handle);
    let state = runner.run();

    assert_eq!(
        recorded(&observed),
        [],
        "the input submitted after the close marker still ran"
    );
    assert_eq!(state, CountdownState { remaining: 5 });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DeepMapInput;

impl Input for DeepMapInput {
    fn sequence(&self) -> NoduleEventSequence {
        0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DeepMapOutput(usize);

impl Output for DeepMapOutput {
    fn sequence(&self) -> NoduleEventSequence {
        0
    }
}

struct DeepMap;

impl Nodule for DeepMap {
    type Input = DeepMapInput;
    type Output = DeepMapOutput;
    type State = ();
    type Dependency = SharedDependency;

    fn receive(
        _input: &Self::Input,
        _state: &mut Self::State,
        _dependency: &Self::Dependency,
    ) -> NodeResult<Self::Output> {
        let mut result = NodeResult::run(std::iter::once(DeepMapOutput(0)));
        for _ in 0..1_000 {
            result = result.map(|output| DeepMapOutput(output.0 + 1));
        }
        result
    }
}

impl NoduleLoop for DeepMap {
    fn input_from_output(
        _output: &Self::Output,
        _state: &Self::State,
        _dependency: &Self::Dependency,
    ) -> Option<Self::Input> {
        None
    }
}
