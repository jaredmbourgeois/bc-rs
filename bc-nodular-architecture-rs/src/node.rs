//! Thread-safe ownership of one nodule's dependency and state.

use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::{NodeResult, Nodule};

/// Nominal override for a node's state-transition function.
pub struct NodeReceive<N>
where
    N: Nodule,
{
    receive: Receive<N>,
}

#[expect(
    clippy::type_complexity,
    reason = "the callback is the nodule's complete receive contract"
)]
enum Receive<N>
where
    N: Nodule,
{
    Nodule,
    Custom(
        Box<
            dyn Fn(&N::Input, &mut N::State, &N::Dependency) -> NodeResult<N::Output>
                + Send
                + Sync
                + 'static,
        >,
    ),
}

impl<N> NodeReceive<N>
where
    N: Nodule,
{
    /// Wraps a custom state-transition function.
    #[must_use]
    pub fn new(
        receive: impl Fn(&N::Input, &mut N::State, &N::Dependency) -> NodeResult<N::Output>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            receive: Receive::Custom(Box::new(receive)),
        }
    }

    pub(crate) fn nodule() -> Self {
        Self {
            receive: Receive::Nodule,
        }
    }

    pub(crate) fn call(
        &self,
        input: &N::Input,
        state: &mut N::State,
        dependency: &N::Dependency,
    ) -> NodeResult<N::Output> {
        match &self.receive {
            Receive::Nodule => N::receive(input, state, dependency),
            Receive::Custom(receive) => receive(input, state, dependency),
        }
    }
}

impl<N> std::fmt::Debug for NodeReceive<N>
where
    N: Nodule,
{
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("NodeReceive(..)")
    }
}

/// Synchronization-capable owner of one nodule's immutable dependency and mutable state.
pub struct Node<N>
where
    N: Nodule,
{
    dependency: N::Dependency,
    receive: NodeReceive<N>,
    state: Mutex<N::State>,
}

impl<N> Node<N>
where
    N: Nodule,
{
    /// Creates a node that dispatches inputs to [`Nodule::receive`].
    #[must_use]
    pub fn new(dependency: N::Dependency, state: N::State) -> Self {
        Self::with_receive(dependency, state, NodeReceive::nodule())
    }

    /// Creates a node with a custom state-transition function.
    #[must_use]
    pub fn with_receive(
        dependency: N::Dependency,
        state: N::State,
        receive: NodeReceive<N>,
    ) -> Self {
        Self {
            dependency,
            receive,
            state: Mutex::new(state),
        }
    }

    /// Applies one input while holding exclusive access to state.
    ///
    /// The receive function runs while the state lock is held. It must not call
    /// `receive`, `with_state`, or `replace_state` on this node because the lock
    /// is not reentrant. If the receive function panics, state changes made
    /// before the panic remain in place; the node recovers the poisoned lock and
    /// does not roll state back.
    pub fn receive(&self, input: &N::Input) -> NodeResult<N::Output> {
        let mut state = self.lock_state();
        self.receive.call(input, &mut state, &self.dependency)
    }

    /// Runs a read-only operation against the current state.
    ///
    /// The operation runs while the state lock is held. It must not call
    /// `receive`, `with_state`, or `replace_state` on this node because the lock
    /// is not reentrant. If the operation panics, later calls recover the
    /// poisoned lock normally.
    pub fn with_state<R>(&self, inspect: impl FnOnce(&N::State) -> R) -> R {
        let state = self.lock_state();
        inspect(&state)
    }

    /// Replaces the current state without changing the dependency.
    pub fn replace_state(&self, state: N::State) {
        *self.lock_state() = state;
    }

    /// Borrows the immutable dependency.
    #[must_use]
    pub const fn dependency(&self) -> &N::Dependency {
        &self.dependency
    }

    fn lock_state(&self) -> MutexGuard<'_, N::State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl<N> std::fmt::Debug for Node<N>
where
    N: Nodule,
{
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Node(..)")
    }
}
