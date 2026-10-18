//! Drivers for one iterative feedback queue.
//!
//! [`NodeLoop`] owns the queue and exposes it two ways: `receive`/`send` drain
//! every consequence before returning, and `submit`/`step` let a caller's own
//! executor drive the same core one bounded piece of work at a time.
//! [`ThreadNodeLoopRunner`] is the second driver, over a bounded standard
//! channel and a thread the caller owns.

use std::collections::VecDeque;
use std::marker::PhantomData;
use std::num::NonZeroUsize;
use std::sync::mpsc::{Receiver, SyncSender};

use crate::node_result::RunStep;
use crate::{NodeResult, NoduleLoop, Run};

/// One piece of work handed to a loop from outside.
pub enum Submission<I, O> {
    /// An input for the nodule to receive.
    Input(I),
    /// An output to observe, and to feed back from.
    Output(O),
    /// An ordered marker closing acceptance once the loop reaches it.
    Close,
}

impl<I, O> Submission<I, O> {
    fn into_input(self) -> I {
        match self {
            Self::Input(input) => input,
            _ => unreachable!("a submission changed variants while being sent"),
        }
    }

    fn into_output(self) -> O {
        match self {
            Self::Output(output) => output,
            _ => unreachable!("a submission changed variants while being sent"),
        }
    }
}

impl<I, O> std::fmt::Debug for Submission<I, O> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Input(_) => "Submission::Input(..)",
            Self::Output(_) => "Submission::Output(..)",
            Self::Close => "Submission::Close",
        })
    }
}

/// Whether a step found work to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// The step performed one piece of work.
    Worked,
    /// The queue was empty, so the step did nothing.
    Idle,
}

/// Whether a loop still accepts new submissions.
///
/// A driver has to act on this: once acceptance closes, everything submitted
/// afterwards is discarded in silence, so ignoring the answer loses work rather
/// than reporting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use = "a closed loop discards later submissions; the driver must stop offering work"]
pub enum Acceptance {
    /// The loop accepts further submissions.
    Open,
    /// The loop has reached a close marker. This submission was discarded
    /// unless it was the marker itself; work already accepted still drains.
    Closed,
}

enum Work<I, O> {
    Input(I),
    Output(O),
    Run(Run<O>),
}

struct WorkQueue<N>
where
    N: NoduleLoop,
{
    dependency: N::Dependency,
    state: N::State,
    pending: VecDeque<Work<N::Input, N::Output>>,
    /// Cleared by the first close marker to arrive. Acceptance is the queue's
    /// own state rather than each driver's, so every driver reports and honours
    /// the same contract instead of inventing one.
    accepting: bool,
    nodule: PhantomData<fn() -> N>,
}

impl<N> WorkQueue<N>
where
    N: NoduleLoop,
    N::Output: Send + 'static,
{
    fn new(dependency: N::Dependency, state: N::State) -> Self {
        Self {
            dependency,
            state,
            pending: VecDeque::new(),
            accepting: true,
            nodule: PhantomData,
        }
    }

    /// Queues one submission, or discards it once acceptance has closed.
    ///
    /// Reports the acceptance state *after* this submission, so the close
    /// marker itself and everything behind it both answer `Closed`.
    fn accept(&mut self, submission: Submission<N::Input, N::Output>) -> Acceptance {
        if !self.accepting {
            return Acceptance::Closed;
        }
        match submission {
            Submission::Input(input) => self.pending.push_back(Work::Input(input)),
            Submission::Output(output) => self.pending.push_back(Work::Output(output)),
            Submission::Close => {
                self.accepting = false;
                return Acceptance::Closed;
            }
        }
        Acceptance::Open
    }

    const fn accepting(&self) -> bool {
        self.accepting
    }

    fn close(&mut self) {
        self.accepting = false;
    }

    fn process_one(&mut self, send_output: &mut dyn FnMut(&N::Output)) -> bool {
        let Some(work) = self.pending.pop_front() else {
            return false;
        };

        match work {
            Work::Input(input) => {
                let result = N::receive(&input, &mut self.state, &self.dependency);
                self.enqueue_result(result);
            }
            Work::Output(output) => {
                send_output(&output);
                if let Some(input) = N::input_from_output(&output, &self.state, &self.dependency) {
                    self.pending.push_front(Work::Input(input));
                }
            }
            Work::Run(mut run) => match run.advance() {
                RunStep::Output(output) => {
                    self.pending.push_front(Work::Run(run));
                    self.pending.push_front(Work::Output(output));
                }
                RunStep::Continue => self.pending.push_front(Work::Run(run)),
                RunStep::Complete => {}
            },
        }
        true
    }

    fn clear(&mut self) {
        self.pending.clear();
    }

    fn enqueue_result(&mut self, result: NodeResult<N::Output>) {
        match result {
            NodeResult::None => {}
            NodeResult::Output(output) => self.pending.push_front(Work::Output(output)),
            NodeResult::Outputs(outputs) => self.enqueue_outputs(outputs),
            NodeResult::Run(run) => self.pending.push_front(Work::Run(run)),
        }
    }

    fn enqueue_outputs(&mut self, outputs: Vec<N::Output>) {
        for output in outputs.into_iter().rev() {
            self.pending.push_front(Work::Output(output));
        }
    }

    fn state(&self) -> &N::State {
        &self.state
    }

    fn into_state(self) -> N::State {
        self.state
    }
}

/// Performs one piece of queued work, failing the caller permanently on panic.
///
/// Shared by both drivers so each can choose its own observer bounds.
fn guarded_step<N>(
    queue: &mut WorkQueue<N>,
    send_output: &mut dyn FnMut(&N::Output),
    failed: &mut bool,
) -> Progress
where
    N: NoduleLoop,
    N::Output: Send + 'static,
{
    assert!(
        !*failed,
        "the node loop is failed after a previous dispatch panic"
    );

    let mut guard = FailLoopOnUnwind {
        queue,
        failed,
        armed: true,
    };
    let worked = guard.queue.process_one(send_output);
    guard.armed = false;

    if worked {
        Progress::Worked
    } else {
        Progress::Idle
    }
}

/// Fails a loop permanently if the work it is guarding unwinds.
///
/// The panic propagates untouched; only the cleanup that used to sit in a
/// `catch_unwind` arm moves here, where it costs the working path nothing.
struct FailLoopOnUnwind<'guard, N>
where
    N: NoduleLoop,
    N::Output: Send + 'static,
{
    queue: &'guard mut WorkQueue<N>,
    failed: &'guard mut bool,
    armed: bool,
}

impl<N> Drop for FailLoopOnUnwind<'_, N>
where
    N: NoduleLoop,
    N::Output: Send + 'static,
{
    fn drop(&mut self) {
        if self.armed {
            *self.failed = true;
            self.queue.clear();
        }
    }
}

/// Deterministic driver for a feedback-capable nodule, and the queue itself.
///
/// [`receive`](Self::receive) and [`send`](Self::send) return only after all
/// work they produced has been processed. [`submit`](Self::submit) and
/// [`step`](Self::step) expose the same core to a caller's own executor, one
/// bounded piece of work at a time. The type uses ordinary mutable ownership
/// and contains no lock.
pub struct NodeLoop<'output, N>
where
    N: NoduleLoop,
{
    queue: WorkQueue<N>,
    #[expect(
        clippy::type_complexity,
        reason = "the boxed observer keeps the public loop type independent of its callback"
    )]
    send_output: Box<dyn FnMut(&N::Output) + 'output>,
    failed: bool,
}

impl<N> NodeLoop<'static, N>
where
    N: NoduleLoop,
    N::Output: Send + 'static,
{
    /// Creates a loop that discards externally observed outputs while retaining feedback.
    #[must_use]
    pub fn new(dependency: N::Dependency, state: N::State) -> Self {
        Self::with_output(dependency, state, |_| {})
    }
}

impl<'output, N> NodeLoop<'output, N>
where
    N: NoduleLoop,
    N::Output: Send + 'static,
{
    /// Creates a loop that observes every output before applying its feedback mapping.
    #[must_use]
    pub fn with_output(
        dependency: N::Dependency,
        state: N::State,
        send_output: impl FnMut(&N::Output) + 'output,
    ) -> Self {
        Self {
            queue: WorkQueue::new(dependency, state),
            send_output: Box::new(send_output),
            failed: false,
        }
    }

    /// Accepts an input and processes it and all causal work before returning.
    ///
    /// A closed loop discards the input. The acceptance answer is dropped rather
    /// than returned because the caller closing this loop and the caller calling
    /// this method are the same caller, holding the same `&mut`: it already knows.
    /// A driver reading submissions from elsewhere must use [`submit`](Self::submit)
    /// and act on what it returns.
    pub fn receive(&mut self, input: N::Input) {
        let _closed_loops_discard = self.submit(Submission::Input(input));
        self.drain();
    }

    /// Injects an output and processes it and all causal work before returning.
    ///
    /// A closed loop discards the output, for the reason [`receive`](Self::receive)
    /// gives.
    pub fn send(&mut self, output: N::Output) {
        let _closed_loops_discard = self.submit(Submission::Output(output));
        self.drain();
    }

    /// Queues one submission, or discards it once acceptance has closed.
    ///
    /// Work already accepted still drains after a [`Submission::Close`]; the
    /// marker itself and every submission behind it answer
    /// [`Acceptance::Closed`], so a driver reading work from elsewhere learns to
    /// stop offering it rather than feeding a queue that drops it.
    ///
    /// # Panics
    ///
    /// Panics if a previous step panicked, which fails the loop permanently.
    pub fn submit(&mut self, submission: Submission<N::Input, N::Output>) -> Acceptance {
        assert!(
            !self.failed,
            "the node loop is failed after a previous dispatch panic"
        );
        self.queue.accept(submission)
    }

    /// Performs at most one piece of queued work.
    ///
    /// # Panics
    ///
    /// Resumes a transition, producer, transform, feedback, or observer panic
    /// after clearing pending causal work and failing the loop permanently.
    pub fn step(&mut self) -> Progress {
        guarded_step(&mut self.queue, self.send_output.as_mut(), &mut self.failed)
    }

    /// Performs up to `quantum` pieces of queued work.
    ///
    /// Reports [`Progress::Worked`] when it did anything at all, so a caller's
    /// executor can yield between quanta without losing track of an empty queue.
    ///
    /// # Panics
    ///
    /// Propagates a step panic exactly as [`step`](Self::step) does.
    pub fn step_quantum(&mut self, quantum: NonZeroUsize) -> Progress {
        let mut progress = Progress::Idle;
        for _ in 0..quantum.get() {
            if self.step() == Progress::Idle {
                break;
            }
            progress = Progress::Worked;
        }
        progress
    }

    /// Borrows the current state after the latest completed drain.
    #[must_use]
    pub fn state(&self) -> &N::State {
        self.queue.state()
    }

    /// Consumes the loop and returns its final state.
    #[must_use]
    pub fn into_state(self) -> N::State {
        self.queue.into_state()
    }

    fn drain(&mut self) {
        while self.step() == Progress::Worked {}
    }
}

impl<N> std::fmt::Debug for NodeLoop<'_, N>
where
    N: NoduleLoop,
{
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("NodeLoop(..)")
    }
}

/// Error returned when a runner rejects a submission after closing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadNodeLoopClosed<T> {
    rejected: T,
}

impl<T> ThreadNodeLoopClosed<T> {
    /// Borrows the rejected input or output.
    #[must_use]
    pub const fn rejected(&self) -> &T {
        &self.rejected
    }

    /// Recovers the rejected input or output.
    #[must_use]
    pub fn into_inner(self) -> T {
        self.rejected
    }
}

impl<T> std::fmt::Display for ThreadNodeLoopClosed<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("the node-loop runner is closed")
    }
}

impl<T> std::error::Error for ThreadNodeLoopClosed<T> where T: std::fmt::Debug {}

/// Cloneable queue handle for submitting source inputs and outputs.
///
/// A successful send means the item was accepted by the bounded queue, not that
/// it was processed. Sending blocks while the queue is full. Join the thread
/// running the paired [`ThreadNodeLoopRunner`] to observe completion.
pub struct ThreadNodeLoopHandle<N>
where
    N: NoduleLoop,
{
    sender: SyncSender<Submission<N::Input, N::Output>>,
}

impl<N> ThreadNodeLoopHandle<N>
where
    N: NoduleLoop,
    N::Input: Send + 'static,
    N::Output: Send + 'static,
{
    /// Queues an input, blocking while the queue is full.
    ///
    /// # Errors
    ///
    /// Returns the rejected input when the runner has closed.
    pub fn receive(&self, input: N::Input) -> Result<(), ThreadNodeLoopClosed<N::Input>> {
        self.sender
            .send(Submission::Input(input))
            .map_err(|error| ThreadNodeLoopClosed {
                rejected: error.0.into_input(),
            })
    }

    /// Queues an output, blocking while the queue is full.
    ///
    /// # Errors
    ///
    /// Returns the rejected output when the runner has closed.
    pub fn send(&self, output: N::Output) -> Result<(), ThreadNodeLoopClosed<N::Output>> {
        self.sender
            .send(Submission::Output(output))
            .map_err(|error| ThreadNodeLoopClosed {
                rejected: error.0.into_output(),
            })
    }

    /// Queues an ordered graceful-close marker.
    ///
    /// The runner closes acceptance when it reaches the marker after earlier
    /// causal work. Items already accepted behind the marker are still drained.
    ///
    /// # Errors
    ///
    /// Returns a closed error when the runner has already closed acceptance.
    pub fn close(&self) -> Result<(), ThreadNodeLoopClosed<()>> {
        self.sender
            .send(Submission::Close)
            .map_err(|_| ThreadNodeLoopClosed { rejected: () })
    }
}

impl<N> Clone for ThreadNodeLoopHandle<N>
where
    N: NoduleLoop,
{
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
        }
    }
}

impl<N> std::fmt::Debug for ThreadNodeLoopHandle<N>
where
    N: NoduleLoop,
{
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ThreadNodeLoopHandle(..)")
    }
}

/// Caller-owned runner that drains a bounded standard channel through the core.
///
/// The runner spawns no thread: the caller decides where
/// [`run`](Self::run) executes. `run` waits for every handle to drop;
/// [`close_and_drain`](Self::close_and_drain) closes acceptance and drains work
/// already accepted even when handles remain alive.
pub struct ThreadNodeLoopRunner<N>
where
    N: NoduleLoop,
{
    queue: WorkQueue<N>,
    #[expect(
        clippy::type_complexity,
        reason = "the boxed observer keeps the public runner type independent of its callback"
    )]
    send_output: Box<dyn FnMut(&N::Output) + Send + 'static>,
    receiver: Receiver<Submission<N::Input, N::Output>>,
    failed: bool,
}

impl<N> ThreadNodeLoopRunner<N>
where
    N: NoduleLoop,
    N::Input: Send + 'static,
    N::Output: Send + 'static,
    N::State: Send + 'static,
    N::Dependency: Send + 'static,
{
    /// Creates a runner that discards externally observed outputs while retaining feedback.
    #[must_use]
    pub fn new(
        dependency: N::Dependency,
        state: N::State,
        capacity: NonZeroUsize,
    ) -> (ThreadNodeLoopHandle<N>, Self) {
        Self::with_output(dependency, state, capacity, |_| {})
    }

    /// Creates a queue handle and its caller-owned output-observing runner.
    #[must_use]
    pub fn with_output(
        dependency: N::Dependency,
        state: N::State,
        capacity: NonZeroUsize,
        send_output: impl FnMut(&N::Output) + Send + 'static,
    ) -> (ThreadNodeLoopHandle<N>, Self) {
        let (sender, receiver) = std::sync::mpsc::sync_channel(capacity.get());
        (
            ThreadNodeLoopHandle { sender },
            Self {
                queue: WorkQueue::new(dependency, state),
                send_output: Box::new(send_output),
                receiver,
                failed: false,
            },
        )
    }

    /// Drains work until every handle drops, then returns final state.
    ///
    /// # Panics
    ///
    /// Propagates a transition, producer, transform, feedback, or observer
    /// panic to this thread's caller, after delivering earlier outputs.
    #[must_use]
    pub fn run(mut self) -> N::State {
        while self.queue.accepting() {
            if self.step() == Progress::Worked {
                continue;
            }
            let Ok(submission) = self.receiver.recv() else {
                return self.queue.into_state();
            };
            let _closed_ends_the_loop = self.queue.accept(submission);
        }
        self.drain_accepted()
    }

    /// Closes acceptance, drains every already accepted item, and returns final state.
    ///
    /// # Panics
    ///
    /// Propagates a step panic exactly as [`run`](Self::run) does.
    #[must_use]
    pub fn close_and_drain(mut self) -> N::State {
        // A handle's `receive` returning `Ok` is what acceptance means to a
        // sender, so whatever is already in the channel has been accepted and is
        // taken up here. An ordered close marker sitting among it stops that
        // intake exactly where it sits, which is the same rule `run` follows.
        while self.queue.accepting() {
            let Ok(submission) = self.receiver.try_recv() else {
                break;
            };
            let _closed_stops_the_intake = self.queue.accept(submission);
        }
        self.queue.close();
        self.drain_accepted()
    }

    fn step(&mut self) -> Progress {
        guarded_step(&mut self.queue, self.send_output.as_mut(), &mut self.failed)
    }

    /// Finishes the work already accepted and gives up the state.
    ///
    /// Reached only once acceptance has closed, so the channel is left alone
    /// rather than drained into a queue that would discard it.
    fn drain_accepted(mut self) -> N::State {
        while self.step() == Progress::Worked {}
        self.queue.into_state()
    }
}

impl<N> std::fmt::Debug for ThreadNodeLoopRunner<N>
where
    N: NoduleLoop,
{
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ThreadNodeLoopRunner(..)")
    }
}
