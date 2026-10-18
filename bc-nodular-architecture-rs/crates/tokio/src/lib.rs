#![deny(missing_docs)]
#![doc = include_str!("../README.md")]

use std::num::NonZeroUsize;

use bc_nodular_architecture::{Acceptance, NodeLoop, Nodule, NoduleLoop, Progress, Submission};

/// Channel receiver a [`run`] call drains submissions from.
pub type Submissions<N> =
    tokio::sync::mpsc::Receiver<Submission<<N as Nodule>::Input, <N as Nodule>::Output>>;

/// Channel sender feeding a [`run`] call.
pub type Submitter<N> =
    tokio::sync::mpsc::Sender<Submission<<N as Nodule>::Input, <N as Nodule>::Output>>;

/// Pieces of queued work performed between yields when a caller has no reason
/// to pick its own bound.
pub const DEFAULT_QUANTUM: NonZeroUsize = NonZeroUsize::new(64).expect("64 is not zero");

/// Creates the submission channel a [`run`] call drains.
///
/// A thin pass-through to [`tokio::sync::mpsc::channel`] that fixes the message
/// type, so a caller never has to spell the [`Submission`] generics itself. The
/// channel is bounded: a full queue makes senders wait, which is the
/// backpressure the core deliberately does not implement.
#[must_use]
pub fn channel<N>(capacity: NonZeroUsize) -> (Submitter<N>, Submissions<N>)
where
    N: NoduleLoop,
{
    tokio::sync::mpsc::channel(capacity.get())
}

/// Drives a node loop from a tokio channel until it stops accepting work.
///
/// Alternates between bounded quanta of queued work and yielding to the
/// runtime, so a loop with a deep cascade in front of it never starves its
/// neighbours. Submissions that have arrived are taken up between quanta rather
/// than only when the queue runs dry, so a long-running cascade does not hold
/// the channel shut against its senders. When there is nothing left to do the
/// call awaits the next submission rather than spinning.
///
/// Returns the loop's final state once every sender has dropped, or once a
/// [`Submission::Close`] closes acceptance — whichever comes first. In both
/// cases the work already accepted drains before the call returns; after a
/// close the channel is left alone rather than drained into a loop that would
/// discard it.
///
/// The returned future is [`Send`] only when the loop's output observer is,
/// which [`NodeLoop`] does not require — it is deliberately usable with a
/// non-`Send` observer so a wasm consumer can hold one. Drive this on a
/// current-thread runtime, a [`tokio::task::LocalSet`], or through
/// [`tokio::runtime::Handle::block_on`] inside a blocking task. A loop that
/// genuinely has to move between threads wants the core's own
/// `ThreadNodeLoopRunner` instead, which requires a `Send` observer and needs
/// no runtime at all.
///
/// # Panics
///
/// Propagates a transition, producer, transform, feedback, or observer panic
/// exactly as [`NodeLoop::step_quantum`] does, after failing the loop
/// permanently. The loop is not resumable afterwards.
pub async fn run<N>(
    mut node_loop: NodeLoop<'_, N>,
    mut submissions: Submissions<N>,
    quantum: NonZeroUsize,
) -> N::State
where
    N: NoduleLoop,
    N::Output: Send + 'static,
{
    loop {
        // Take up whatever has arrived before working. Waiting for the queue to
        // run dry would hold a bounded channel shut for the length of a cascade,
        // and shut forever against a run that never ends.
        while let Ok(submission) = submissions.try_recv() {
            if node_loop.submit(submission) == Acceptance::Closed {
                return drain(node_loop, quantum).await;
            }
        }

        if node_loop.step_quantum(quantum) == Progress::Worked {
            tokio::task::yield_now().await;
            continue;
        }

        match submissions.recv().await {
            Some(submission) => {
                if node_loop.submit(submission) == Acceptance::Closed {
                    return drain(node_loop, quantum).await;
                }
            }
            None => return drain(node_loop, quantum).await,
        }
    }
}

/// Finishes the work a loop has already accepted and gives up its state.
///
/// Keeps the quantum-and-yield discipline of the working path: a close arriving
/// in front of a deep cascade must not hold the runtime for the length of it.
async fn drain<N>(mut node_loop: NodeLoop<'_, N>, quantum: NonZeroUsize) -> N::State
where
    N: NoduleLoop,
    N::Output: Send + 'static,
{
    while node_loop.step_quantum(quantum) == Progress::Worked {
        tokio::task::yield_now().await;
    }
    node_loop.into_state()
}
