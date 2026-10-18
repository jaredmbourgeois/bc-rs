//! Contracts for nodule events, state, dependencies, and transitions.

use crate::{NodeResult, NoduleEventSequence, SharedDependency};

/// Input event accepted by a nodule.
///
/// A sequence method is required.
///
/// <!-- fidelity-doctest:input-no-sequence -->
/// ```compile_fail,E0046
/// use bc_nodular_architecture::Input;
///
/// #[derive(PartialEq)]
/// struct NoSequence;
///
/// impl Input for NoSequence {}
/// ```
///
/// Inputs must support partial equality.
///
/// <!-- fidelity-doctest:input-not-equatable -->
/// ```compile_fail,E0277
/// use bc_nodular_architecture::{Input, NoduleEventSequence};
///
/// struct NotEquatable;
///
/// impl Input for NotEquatable {
///     fn sequence(&self) -> NoduleEventSequence { 0 }
/// }
/// ```
///
/// Implementations are nominal; primitive integers are not inputs.
///
/// <!-- fidelity-doctest:input-integer -->
/// ```compile_fail,E0277
/// use bc_nodular_architecture::Input;
///
/// fn require_input<T: Input>() {}
///
/// require_input::<i32>();
/// ```
pub trait Input: PartialEq {
    /// Monotonic sequence used to order the event.
    fn sequence(&self) -> NoduleEventSequence;
}

/// Output event produced by a nodule.
///
/// A sequence method is required.
///
/// <!-- fidelity-doctest:output-no-sequence -->
/// ```compile_fail,E0046
/// use bc_nodular_architecture::Output;
///
/// #[derive(PartialEq)]
/// struct NoSequence;
///
/// impl Output for NoSequence {}
/// ```
///
/// Outputs must support partial equality.
///
/// <!-- fidelity-doctest:output-not-equatable -->
/// ```compile_fail,E0277
/// use bc_nodular_architecture::{NoduleEventSequence, Output};
///
/// struct NotEquatable;
///
/// impl Output for NotEquatable {
///     fn sequence(&self) -> NoduleEventSequence { 0 }
/// }
/// ```
pub trait Output: PartialEq {
    /// Monotonic sequence used to order the event.
    fn sequence(&self) -> NoduleEventSequence;
}

/// Input that can be constructed directly from an output.
///
/// Implementing [`Input`] alone does not provide this conversion.
///
/// <!-- fidelity-doctest:input-from-output-missing -->
/// ```compile_fail,E0277
/// use bc_nodular_architecture::{Input, InputFromOutput, NoduleEventSequence, Output};
///
/// #[derive(PartialEq)]
/// struct FeedbackInput;
///
/// impl Input for FeedbackInput {
///     fn sequence(&self) -> NoduleEventSequence { 0 }
/// }
///
/// #[derive(PartialEq)]
/// struct FeedbackOutput;
///
/// impl Output for FeedbackOutput {
///     fn sequence(&self) -> NoduleEventSequence { 0 }
/// }
///
/// fn require_conversion<I, O>()
/// where
///     I: InputFromOutput<O>,
///     O: Output,
/// {}
///
/// require_conversion::<FeedbackInput, FeedbackOutput>();
/// ```
pub trait InputFromOutput<O>: Input + Sized
where
    O: Output,
{
    /// Converts an output into feedback when the output is relevant.
    fn from_output(output: O) -> Option<Self>;
}

/// Equality-comparable nodule state.
///
/// <!-- fidelity-doctest:state-not-equatable -->
/// ```compile_fail,E0277
/// use bc_nodular_architecture::State;
///
/// struct NotEquatable;
///
/// fn require_state<T: State>() {}
///
/// require_state::<NotEquatable>();
/// ```
pub trait State: PartialEq {}

impl<T> State for T where T: PartialEq {}

/// Dependency bag that exposes the family-wide shared dependencies.
///
/// The shared dependency accessor is required.
///
/// <!-- fidelity-doctest:dependency-no-shared -->
/// ```compile_fail,E0046
/// use bc_nodular_architecture::Dependency;
///
/// struct NoDependencyShared;
///
/// impl Dependency for NoDependencyShared {}
/// ```
pub trait Dependency {
    /// Returns the shared dependency values.
    fn shared(&self) -> &SharedDependency;
}

/// Pure state transition with distinct input and output event types.
pub trait Nodule {
    /// Input event accepted by this nodule.
    type Input: Input;
    /// Output event produced by this nodule.
    type Output: Output;
    /// Mutable state owned by a node loop.
    type State: State;
    /// Immutable dependencies injected at construction.
    type Dependency: Dependency;

    /// Applies one input to state and describes the resulting outputs.
    fn receive(
        input: &Self::Input,
        state: &mut Self::State,
        dependency: &Self::Dependency,
    ) -> NodeResult<Self::Output>;
}
