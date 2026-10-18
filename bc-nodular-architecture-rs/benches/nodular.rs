//! Workload timings for the node runtime, the loop cascade, and the `Run` pipeline.
//!
//! Runs without a benchmark framework so the crate keeps its empty dependency
//! graph. Each row prints nanoseconds per operation, where an operation is the
//! unit named in the row: one `receive`, or one delivered output. The C++
//! figures the README compares against come from a harness outside this
//! repository, built with `clang++ -std=c++20 -O2`.

#![expect(
    clippy::cast_precision_loss,
    reason = "operation counts are small enough to convert exactly"
)]

use std::hint::black_box;
use std::sync::Barrier;
use std::time::{Duration, Instant};

use bc_date_time::{Epoch, fixed_epoch_source};
use bc_nodular_architecture::{
    CombinedDependency, CombinedInput, CombinedNodule, CombinedState, Input, LinkedDependency,
    LinkedNodule, LinkedState, Node, NodeLoop, NodeResult, Nodule, NoduleEventSequence,
    NoduleLinking, NoduleLoop, Output, RunStep, SharedDependency,
};

fn dependency() -> SharedDependency {
    SharedDependency::new(fixed_epoch_source(Epoch::ZERO))
}

// ---------------------------------------------------------------- fixtures

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Tick;

impl Input for Tick {
    fn sequence(&self) -> NoduleEventSequence {
        0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Count(usize);

impl Output for Count {
    fn sequence(&self) -> NoduleEventSequence {
        0
    }
}

/// Counts inputs and produces nothing: isolates dispatch and the state lock.
struct Counter;

impl Nodule for Counter {
    type Input = Tick;
    type Output = Count;
    type State = usize;
    type Dependency = SharedDependency;

    fn receive(
        _input: &Self::Input,
        state: &mut Self::State,
        _dependency: &Self::Dependency,
    ) -> NodeResult<Self::Output> {
        *state += 1;
        NodeResult::none()
    }
}

/// Emits one output per input: isolates the immediate-output path.
struct Emitter;

impl Nodule for Emitter {
    type Input = Tick;
    type Output = Count;
    type State = usize;
    type Dependency = SharedDependency;

    fn receive(
        _input: &Self::Input,
        state: &mut Self::State,
        _dependency: &Self::Dependency,
    ) -> NodeResult<Self::Output> {
        *state += 1;
        NodeResult::output(Count(*state))
    }
}

/// Pipes every emitter output into the counter.
struct EmitterToCounter;

impl NoduleLinking for EmitterToCounter {
    type FirstNodule = Emitter;
    type SecondNodule = Counter;

    fn second_input_from_first_output(
        _first_output: &Count,
        _state: &LinkedState<usize, usize>,
        _dependency: &LinkedDependency<SharedDependency, SharedDependency>,
    ) -> Option<Tick> {
        Some(Tick)
    }

    fn continue_first_run(
        _first_output: &Count,
        _dependency: &LinkedDependency<SharedDependency, SharedDependency>,
    ) -> Option<Count> {
        None
    }
}

struct Countdown;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Remaining(usize);

impl Output for Remaining {
    fn sequence(&self) -> NoduleEventSequence {
        0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CountdownState {
    remaining: usize,
}

impl Nodule for Countdown {
    type Input = Tick;
    type Output = Remaining;
    type State = CountdownState;
    type Dependency = SharedDependency;

    fn receive(
        _input: &Self::Input,
        state: &mut Self::State,
        _dependency: &Self::Dependency,
    ) -> NodeResult<Self::Output> {
        state.remaining -= 1;
        NodeResult::output(Remaining(state.remaining))
    }
}

impl NoduleLoop for Countdown {
    fn input_from_output(
        output: &Self::Output,
        _state: &Self::State,
        _dependency: &Self::Dependency,
    ) -> Option<Self::Input> {
        (output.0 > 0).then_some(Tick)
    }
}

// ---------------------------------------------------------------- reporting

struct Report {
    rows: Vec<(String, u64, f64)>,
}

impl Report {
    fn new() -> Self {
        Self { rows: Vec::new() }
    }

    fn record(&mut self, workload: &str, operations: u64, elapsed: Duration) {
        let nanoseconds = elapsed.as_secs_f64() * 1e9 / operations as f64;
        self.rows
            .push((workload.to_owned(), operations, nanoseconds));
    }

    fn print(&self) {
        let width = self.rows.iter().map(|row| row.0.len()).max().unwrap_or(0);
        println!(
            "\n{:<width$}  {:>12}  {:>10}",
            "workload", "operations", "ns/op"
        );
        for (workload, operations, nanoseconds) in &self.rows {
            println!("{workload:<width$}  {operations:>12}  {nanoseconds:>10.2}");
        }
    }
}

// ---------------------------------------------------------------- workloads

/// W1 — uncontended `Node::receive`.
fn node_receive(report: &mut Report) {
    const OPERATIONS: u64 = 1_000_000;

    let node = Node::<Counter>::new(dependency(), 0);
    let start = Instant::now();
    for _ in 0..OPERATIONS {
        let _ = black_box(node.receive(black_box(&Tick)));
    }
    report.record("W1 node receive (1 thread)", OPERATIONS, start.elapsed());
}

/// W2 — `Node::receive` contended by four threads, the `Node.ThreadSafety` shape.
fn node_receive_contended(report: &mut Report) {
    const THREADS: usize = 4;
    const PER_THREAD: u64 = 250_000;

    let node = Node::<Counter>::new(dependency(), 0);
    let ready = Barrier::new(THREADS + 1);
    let elapsed = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..THREADS)
            .map(|_| {
                scope.spawn(|| {
                    ready.wait();
                    for _ in 0..PER_THREAD {
                        let _ = black_box(node.receive(black_box(&Tick)));
                    }
                })
            })
            .collect();

        ready.wait();
        let start = Instant::now();
        for worker in workers {
            worker.join().expect("a contended worker panicked");
        }
        start.elapsed()
    });

    report.record(
        "W2 node receive (4 threads)",
        THREADS as u64 * PER_THREAD,
        elapsed,
    );
}

/// W3 — one `NodeLoop` cascade of feedback transitions.
fn loop_feedback_cascade(report: &mut Report) {
    const DEPTH: usize = 100_000;

    let mut delivered = 0_u64;
    let elapsed = {
        let mut node_loop = NodeLoop::<Countdown>::with_output(
            dependency(),
            CountdownState { remaining: DEPTH },
            |output| {
                black_box(output);
                delivered += 1;
            },
        );
        let start = Instant::now();
        node_loop.receive(Tick);
        start.elapsed()
    };

    assert_eq!(delivered, DEPTH as u64);
    report.record("W3 loop feedback cascade", DEPTH as u64, elapsed);
}

/// W4 — `Run` outputs through a chain of map stages.
fn run_pipeline(report: &mut Report, stages: usize) {
    const OUTPUTS: usize = 100_000;

    let mut result = NodeResult::run((0..OUTPUTS).map(Count));
    for _ in 0..stages {
        result = result.map(|output| Count(output.0 + 1));
    }
    let NodeResult::Run(mut run) = result else {
        unreachable!("a mapped run stays a run");
    };

    let start = Instant::now();
    let mut delivered = 0_u64;
    loop {
        match run.advance() {
            RunStep::Output(output) => {
                black_box(output);
                delivered += 1;
            }
            RunStep::Continue => {}
            RunStep::Complete => break,
        }
    }
    let elapsed = start.elapsed();

    assert_eq!(delivered, OUTPUTS as u64);
    report.record(
        &format!("W4 run pipeline ({stages} stages)"),
        OUTPUTS as u64,
        elapsed,
    );
}

/// W5 — composed nodules on the immediate-output path.
fn composition(report: &mut Report) {
    const OPERATIONS: u64 = 1_000_000;

    let combined = Node::<CombinedNodule<Emitter, Counter>>::new(
        CombinedDependency {
            first: dependency(),
            second: dependency(),
        },
        CombinedState {
            first: 0,
            second: 0,
        },
    );
    let start = Instant::now();
    for _ in 0..OPERATIONS {
        let _ = black_box(combined.receive(black_box(&CombinedInput::First(Tick))));
    }
    report.record("W5 combined receive", OPERATIONS, start.elapsed());

    let linked = Node::<LinkedNodule<EmitterToCounter>>::new(
        LinkedDependency {
            first: dependency(),
            second: dependency(),
        },
        LinkedState {
            first: 0,
            second: 0,
        },
    );
    let start = Instant::now();
    for _ in 0..OPERATIONS {
        let _ = black_box(linked.receive(black_box(&Tick)));
    }
    report.record("W5 linked receive", OPERATIONS, start.elapsed());
}

fn main() {
    let mut report = Report::new();
    node_receive(&mut report);
    node_receive_contended(&mut report);
    loop_feedback_cascade(&mut report);
    for stages in [0, 1, 8] {
        run_pipeline(&mut report, stages);
    }
    composition(&mut report);
    report.print();
}
